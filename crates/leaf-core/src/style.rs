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
//!
//! The one place core does carry a paint value is the presentation vocabulary
//! the author writes by hand: a `data-size` is a *name* ([`SizeStep`]) where a
//! name will do and a measurement ([`FontSize::Points`]) where the author needed
//! exactness, and the open type beside each closed enum — [`FontSize`],
//! [`LineHeight`], [`TextColor`], [`FontFace`] — is where the two meet. A name
//! outlives a theme change and a value does not, which is the trade the author
//! makes knowingly; see `docs/proposals/exact-presentation-values.md`.

use std::borrow::Cow;

/// The value `key` carries in an attribute list, or `None` where the key is
/// absent or bare (`<span data-size>`, which names nothing).
///
/// The one line every `from_attrs` in this module opens with. It takes the
/// attribute list rather than the node so the module stays free of twig as well
/// as of any toolkit; the pairs are plain `String`s, and the WYSIWYG builder,
/// the source builder and the caret queries all hand over the same
/// `node.attrs`.
fn attr<'a>(attrs: &'a [(String, Option<String>)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.as_deref())
}

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
///
/// These seven stay the whole of a *highlight's* vocabulary, because they are
/// encoded in twig's Markdown bytes as circle emoji and are twig's to keep
/// closed. A run's foreground has no such constraint and opens: see
/// [`TextColor`], which is one of these names or a hex triple.
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
    /// Reads the list through [`attr`], which is why the pairs are plain
    /// `String`s rather than anything of twig's.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attr(attrs, "data-color").and_then(Self::from_attr)
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
        attr(attrs, "class")?
            .split_whitespace()
            .find_map(Self::from_token)
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
        attr(attrs, "data-line-height").and_then(Self::from_attr)
    }

    /// Read a `data-line-height` value. `None` for anything that is not one of
    /// the three names — an exact ratio is [`LineHeight::Ratio`]'s to read, and
    /// [`LineHeight::from_attr`] is the door that reads both.
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
        attr(attrs, "data-size").and_then(Self::from_attr)
    }

    /// Read a `data-size` value. `None` for a name outside the vocabulary — a
    /// `data-size="14pt"` is a measurement and [`FontSize::Points`]'s to read,
    /// and [`FontSize::from_attr`] is the door that reads both.
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
/// A named family is not in this vocabulary: [`from_attr`](Self::from_attr)
/// answers `None` for it, and it is [`FontFace::Named`]'s to read — the open
/// type beside this one, which the toolbar offers under the four generics.
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
        attr(attrs, "data-font").and_then(Self::from_attr)
    }

    /// Read a `data-font` value. `None` for a concrete family name, which is
    /// [`FontFace::Named`]'s to read; [`FontFace::from_attr`] is the door that
    /// reads both.
    ///
    /// The four generics are matched without regard to case, because they are
    /// CSS keywords and CSS reads a keyword either way — a hand-written
    /// document says `Serif` as readily as `serif`, and a family actually
    /// *named* "Serif" is not a thing anyone has installed. The canonical
    /// spelling [`name`](Self::name) writes back is the lowercase one.
    pub fn from_attr(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|f| value.eq_ignore_ascii_case(f.name()))
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

// ── the exact forms: a value where a name will not do ───────────────────────
//
// Each of the four run- and block-level properties gets an *open* type beside
// its closed enum: the name, or the measurement the name cannot be. The enums
// above are unchanged and still the menus' first offer — a name is portable
// and a value is exact, and the author who takes the second takes the
// portability cost knowingly.
//
// Each open type reads its own key with `from_attrs`, tries the **name first**
// and the value second, and spells what it holds back with `name()`. A value
// the grammar does not cover — `huge`, `14px`, `rgb(…)`, `1.3em` — answers
// `None` exactly as it did before these types existed: carried untouched by the
// document and drawn at the theme's default.

/// A number a presentation value carries, in hundredths — 14 points is `1400`,
/// a ratio of 1.3 is `130`.
///
/// Fixed point rather than an `f32` because a [`Style`] is stamped on every
/// glyph and compared for run-merging, so the type has to be `Eq`, and because
/// two spellings of the same number must be the same value: an author's
/// `14.0pt` and the menu's `14pt` are one size, not two runs. Hundredths is
/// more precision than any menu writes and enough for the `13.25pt` a fitted
/// theme lands on.
///
/// `u16`, so the largest value is 655.35 — past any type size a sheet of paper
/// holds and any line height a document means. A number above it is not a
/// number this vocabulary carries, and reads as `None`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Hundredths(u16);

impl Hundredths {
    /// The number `value` names, rounded to the nearest hundredth. `None` for
    /// anything that is not a positive finite number this can hold — the same
    /// answer the parsers give a value outside the grammar.
    pub fn from_f32(value: f32) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let h = (value * 100.0).round();
        (1.0..=f32::from(u16::MAX))
            .contains(&h)
            .then_some(Self(h as u16))
    }

    /// The number itself — what a frontend lays out with.
    pub fn as_f32(self) -> f32 {
        self.0 as f32 / 100.0
    }

    /// The hundredths themselves, for a frontend that would rather do integer
    /// arithmetic than divide and multiply back.
    pub const fn hundredths(self) -> u16 {
        self.0
    }

    /// Read a decimal — digits, at most one point, no sign and no exponent,
    /// which is the whole of what a word processor's field writes. `None` for
    /// anything else, and for zero: a size or a spacing of nothing is not a
    /// value, it is a mistake.
    ///
    /// CSS's `<number>` wants digits on whichever side of the point it has, so
    /// `.5` is a number and `14.` is not, and this reads them the same way.
    ///
    /// A third decimal place rounds rather than being refused, because the
    /// number a colour picker or a font panel hands back is whatever floating
    /// point made of the slider.
    fn parse(value: &str) -> Option<Self> {
        let (int, frac) = match value.split_once('.') {
            // A trailing bare point is not a `<number>`: `14.` is a typo, and
            // reading it as 14 would write a document the author did not mean.
            Some((_, "")) => return None,
            Some(pair) => pair,
            None => (value, ""),
        };
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        let whole: u32 = if int.is_empty() { 0 } else { int.parse().ok()? };
        // Out of range before the arithmetic rather than after it, so a
        // thousand digits of integer part is a `None` and not an overflow.
        if whole > u32::from(u16::MAX) / 100 {
            return None;
        }
        let digit = |i: usize| frac.as_bytes().get(i).map_or(0, |b| u32::from(b - b'0'));
        let mut h = whole * 100 + digit(0) * 10 + digit(1);
        if digit(2) >= 5 {
            h += 1;
        }
        (1..=u32::from(u16::MAX))
            .contains(&h)
            .then_some(Self(h as u16))
    }
}

impl std::fmt::Display for Hundredths {
    /// The shortest decimal that means this number: `14`, `13.5`, `1.25`. What
    /// a document is written with, so that a value set twice from the same menu
    /// spells the same bytes both times.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (whole, frac) = (self.0 / 100, self.0 % 100);
        match frac {
            0 => write!(f, "{whole}"),
            _ if frac % 10 == 0 => write!(f, "{whole}.{}", frac / 10),
            _ => write!(f, "{whole}.{frac:02}"),
        }
    }
}

/// How large a run is set: a [`SizeStep`] relative to the text around it, or
/// the point size the author asked for.
///
/// The step is what a menu offers first, and is what a document should say
/// where a name will do — it reads as a step up under every theme. The point
/// size is what the author typed and is all it is: 14 points of the sheet on
/// the paginated view, and 14 points before the zoom on screen, which is how
/// every other point size on a page behaves. A heading set to an exact size is
/// that size and not its ramp scaled — the name scales the ramp, the value
/// replaces it.
///
/// Only `pt` is a unit. `px` is a screen's unit and a document is not a screen;
/// `em`, `rem` and `%` are relative, which is what the steps already are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontSize {
    Step(SizeStep),
    Points(Hundredths),
}

impl FontSize {
    /// The size a run's or block's attributes name, if any — twig records it
    /// under `data-size`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attr(attrs, "data-size").and_then(Self::from_attr)
    }

    /// Read a `data-size` value: one of CSS's seven keywords, or a `<number>pt`.
    /// The unit is matched without regard to case, because a hand-written
    /// document says `14PT` as readily as `14pt` and CSS reads both.
    pub fn from_attr(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(step) = SizeStep::from_attr(value) {
            return Some(Self::Step(step));
        }
        let number = value.strip_suffix("pt").or_else(|| {
            let (n, unit) = value.split_at_checked(value.len().checked_sub(2)?)?;
            unit.eq_ignore_ascii_case("pt").then_some(n)
        })?;
        Hundredths::parse(number).map(Self::Points)
    }

    /// A size in points, or `None` for a number this vocabulary cannot carry —
    /// the constructor an *Other…* field calls with what the author typed.
    pub fn points(points: f32) -> Option<Self> {
        Hundredths::from_f32(points).map(Self::Points)
    }

    /// The token twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back: the step's CSS keyword, or the shortest decimal with `pt`
    /// after it.
    pub fn name(self) -> Cow<'static, str> {
        match self {
            Self::Step(step) => Cow::Borrowed(step.name()),
            Self::Points(pt) => Cow::Owned(format!("{pt}pt")),
        }
    }

    /// The step, for a menu asking which of its seven rows to tick — `None`
    /// when the size is an exact one, which the menu shows as its own row.
    pub const fn step(self) -> Option<SizeStep> {
        match self {
            Self::Step(step) => Some(step),
            Self::Points(_) => None,
        }
    }

    /// The point size, or `None` when the size is a step — whose size in points
    /// is the theme's business and not this type's.
    pub fn points_value(self) -> Option<f32> {
        match self {
            Self::Step(_) => None,
            Self::Points(pt) => Some(pt.as_f32()),
        }
    }
}

/// How far apart a block's lines are set: a [`LineSpacing`] from the menu's
/// three, or the ratio the author asked for. [`FontSize`]'s peer, one property
/// along, and with no unit at all — a line height is a multiple.
///
/// A ratio that spells one of the three names *is* that name:
/// [`from_attr`](Self::from_attr) answers `Step(OneHalf)` for `1.50` as well as
/// for `1.5`, so that two spellings of one spacing are one value and the
/// document is rewritten with the name a menu can tick.
///
/// And a ratio of **1 is absence**, exactly as it is for [`LineSpacing`], whose
/// three names begin above it: single spacing is what a block with no
/// `data-line-height` is set at, and a document should not carry a key that
/// says nothing. `from_attr("1")` and `ratio(1.0)` both answer `None`, so a
/// gesture given one clears the key and the query then ticks *Single*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineHeight {
    Step(LineSpacing),
    Ratio(Hundredths),
}

impl LineHeight {
    /// The spacing a block's attributes name, if any — twig records it under
    /// `data-line-height`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attr(attrs, "data-line-height").and_then(Self::from_attr)
    }

    /// Read a `data-line-height` value: one of the three names, or any positive
    /// decimal other than 1, which is absence.
    pub fn from_attr(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(step) = LineSpacing::from_attr(value) {
            return Some(Self::Step(step));
        }
        Hundredths::parse(value).and_then(Self::of)
    }

    /// A ratio, or `None` for a number this vocabulary cannot carry — the
    /// constructor an *Other…* field calls with what the author typed. 1 is one
    /// of those numbers: see the type's note.
    pub fn ratio(ratio: f32) -> Option<Self> {
        Hundredths::from_f32(ratio).and_then(Self::of)
    }

    /// A ratio as the name for it where there is one, and `None` where the
    /// ratio is 1 — see the type's note.
    ///
    /// Compared in hundredths rather than through [`Display`](std::fmt::Display)
    /// and [`LineSpacing::from_attr`], so that reading a spacing does not format
    /// a `String` to throw away.
    fn of(ratio: Hundredths) -> Option<Self> {
        const SINGLE: u16 = 100;
        if ratio.hundredths() == SINGLE {
            return None;
        }
        let step = LineSpacing::ALL
            .into_iter()
            .find(|s| (s.ratio() * 100.0).round() as u16 == ratio.hundredths());
        Some(match step {
            Some(step) => Self::Step(step),
            None => Self::Ratio(ratio),
        })
    }

    /// The token twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back.
    pub fn name(self) -> Cow<'static, str> {
        match self {
            Self::Step(step) => Cow::Borrowed(step.name()),
            Self::Ratio(r) => Cow::Owned(r.to_string()),
        }
    }

    /// The step, for a menu asking which of its three rows to tick — `None` for
    /// an exact ratio, which the menu shows as its own row.
    pub const fn step(self) -> Option<LineSpacing> {
        match self {
            Self::Step(step) => Some(step),
            Self::Ratio(_) => None,
        }
    }

    /// The ratio itself — what a frontend multiplies the theme's line height
    /// by. Unlike [`FontSize`]'s points this always answers, because a step's
    /// ratio is the name read as arithmetic ([`LineSpacing::ratio`]) and not a
    /// theme's choice.
    pub fn as_f32(self) -> f32 {
        match self {
            Self::Step(step) => step.ratio(),
            Self::Ratio(r) => r.as_f32(),
        }
    }
}

/// A run's foreground colour: one of the seven [`MarkColor`] names, or the RGB
/// triple the author asked for.
///
/// A name is two inks, one per appearance, and the theme owns both. A triple is
/// painted as written in the light appearance and in the dark one alike — that
/// is what "exact" means, and the proposal does not soften it with a heuristic.
/// `#rgb` is read and never written; six lowercase digits is the spelling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextColor {
    Named(MarkColor),
    Rgb { r: u8, g: u8, b: u8 },
}

impl TextColor {
    /// The colour a run's or block's attributes name, if any — twig records it
    /// under `data-color`, the key a `mark` node carries its *highlight's*
    /// colour under. The two never collide: a `mark` is a `mark` and a span is
    /// a span.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attr(attrs, "data-color").and_then(Self::from_attr)
    }

    /// Read a `data-color` value: one of the seven names, `#rrggbb`, or the
    /// `#rgb` shorthand a stylesheet author writes (`#f00` is `#ff0000`, each
    /// digit doubled, as CSS expands it).
    pub fn from_attr(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(named) = MarkColor::from_attr(value) {
            return Some(Self::Named(named));
        }
        let hex = value.strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let nib = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 => Some(Self::Rgb {
                r: nib(0)? * 0x11,
                g: nib(1)? * 0x11,
                b: nib(2)? * 0x11,
            }),
            6 => Some(Self::Rgb {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
            }),
            _ => None,
        }
    }

    /// The token twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back: the name, or six lowercase hex digits behind a `#`.
    pub fn name(self) -> Cow<'static, str> {
        match self {
            Self::Named(c) => Cow::Borrowed(c.name()),
            Self::Rgb { r, g, b } => Cow::Owned(format!("#{r:02x}{g:02x}{b:02x}")),
        }
    }

    /// The name, for a palette asking which of its seven swatches to tick —
    /// `None` for an exact triple, which the palette shows as a swatch of its
    /// own.
    pub const fn named(self) -> Option<MarkColor> {
        match self {
            Self::Named(c) => Some(c),
            Self::Rgb { .. } => None,
        }
    }

    /// The triple, or `None` for a name — whose two inks are the theme's.
    pub const fn rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            Self::Named(_) => None,
            Self::Rgb { r, g, b } => Some((r, g, b)),
        }
    }
}

/// The face a run is set in: one of CSS's four generics, or the family the
/// author named.
///
/// A generic opens on every machine and a family name does not, which is the
/// trade stated once in [`FontFamily`]'s own note. A named family is resolved
/// through the platform's font registry by the frontends that have one, and
/// falls back to the body face where it is not installed.
///
/// Owned, because a family name is a `String` and this is what a gesture takes
/// and a query answers — neither is per-glyph. What a *glyph* carries is
/// [`FaceRef`], the `Copy` half, whose name is looked up in the map's
/// [`FaceTable`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FontFace {
    Generic(FontFamily),
    Named(String),
}

impl FontFace {
    /// The face a run's or block's attributes name, if any — twig records it
    /// under `data-font`.
    pub fn from_attrs(attrs: &[(String, Option<String>)]) -> Option<Self> {
        attr(attrs, "data-font").and_then(Self::from_attr)
    }

    /// Read a `data-font` value: one of the four generics, or any other
    /// non-empty string, which is a family name. Trimmed, and nothing else —
    /// a family name is what the author typed, and leaf has no table of real
    /// ones to check it against.
    pub fn from_attr(value: &str) -> Option<Self> {
        let value = value.trim();
        match FontFamily::from_attr(value) {
            Some(generic) => Some(Self::Generic(generic)),
            None => (!value.is_empty()).then(|| Self::Named(value.to_string())),
        }
    }

    /// The token twig spells it with, and what [`from_attr`](Self::from_attr)
    /// reads back — the generic's CSS keyword, or the family name as given.
    pub fn name(&self) -> Cow<'_, str> {
        match self {
            Self::Generic(generic) => Cow::Borrowed(generic.name()),
            Self::Named(name) => Cow::Borrowed(name.as_str()),
        }
    }

    /// The generic, for a menu asking which of its four rows to tick — `None`
    /// for a named family, which the menu shows as a row of its own.
    pub const fn generic(&self) -> Option<FontFamily> {
        match self {
            Self::Generic(generic) => Some(*generic),
            Self::Named(_) => None,
        }
    }
}

/// A named family's id on a glyph — what [`FaceRef::Named`] carries and
/// [`FaceTable::name`] reads back.
///
/// A 32-bit FNV-1a of the family name, and *not* an index, for one reason: a
/// glyph's id has to mean the same thing however its row was built. A row comes
/// from a fresh walk, from a [`crate::wysiwyg::BlockCache`] hit cloned at a
/// shifted offset, or from a previous map a splice kept untouched — and an
/// index into a table that each of those three assembled differently would have
/// the same glyph naming two faces. Derived from the name, nothing has to be
/// remapped and a spliced map's glyphs compare equal to a fresh build's.
///
/// Two family names that hashed alike would draw in one face. That needs about
/// 2¹⁶ distinct families in one document to become likely, and a document with
/// 2¹⁶ families has a different problem.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FaceId(u32);

impl FaceId {
    /// The id a family name has. FNV-1a, written out rather than taken from
    /// `DefaultHasher`, because the value is compared across builds and must
    /// not depend on a hasher's seed or version.
    pub fn of(name: &str) -> Self {
        let mut h: u32 = 0x811c_9dc5;
        for b in name.as_bytes() {
            h ^= u32::from(*b);
            h = h.wrapping_mul(0x0100_0193);
        }
        Self(h)
    }
}

/// The face a *glyph* is set in — [`FontFace`]'s `Copy` half, so that a
/// [`Style`] stays `Copy` and `Eq` and can be stamped on every glyph and
/// compared for run-merging.
///
/// A named family is an id into the map's [`FaceTable`], which the walker
/// interns into as it meets each name. A frontend reads the name back with
/// [`crate::wysiwyg::VisualMap::face_name`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaceRef {
    Generic(FontFamily),
    Named(FaceId),
}

impl FaceRef {
    /// The generic, or `None` for a named family — [`FontFace::generic`]'s peer.
    pub const fn generic(self) -> Option<FontFamily> {
        match self {
            Self::Generic(generic) => Some(generic),
            Self::Named(_) => None,
        }
    }

    /// The id of the named family, or `None` for a generic.
    pub const fn id(self) -> Option<FaceId> {
        match self {
            Self::Generic(_) => None,
            Self::Named(id) => Some(id),
        }
    }
}

/// Every named family a [`crate::wysiwyg::VisualMap`] draws, by the id its
/// glyphs carry — the side table that lets [`Style`] stay `Copy` while a face
/// name stays a `String`.
///
/// Small: one entry per *distinct* family name in the document, which is
/// normally none. A splice may leave an entry no glyph names any more, because
/// the splice reuses the previous map's table rather than rebuilding it from
/// rows it deliberately did not walk; an unused entry costs a string and draws
/// nothing.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct FaceTable {
    /// Ascending by id, so a lookup is a binary search and two tables built
    /// from the same names compare equal whatever order they met them in.
    entries: Vec<(FaceId, String)>,
}

impl FaceTable {
    /// The family name `id` stands for, or `None` for an id from another map —
    /// which a frontend draws in the theme's body face, as it draws a face it
    /// cannot resolve.
    pub fn name(&self, id: FaceId) -> Option<&str> {
        let i = self.entries.binary_search_by_key(&id, |(k, _)| *k).ok()?;
        Some(self.entries[i].1.as_str())
    }

    /// Every family in the table, by id — how a frontend warms a font cache
    /// before it draws.
    pub fn iter(&self) -> impl Iterator<Item = (FaceId, &str)> {
        self.entries.iter().map(|(id, name)| (*id, name.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Record one family, keeping the vec ascending by id — the sorted insert
    /// both doors below are, written once.
    fn insert(&mut self, id: FaceId, name: &str) {
        if let Err(i) = self.entries.binary_search_by_key(&id, |(k, _)| *k) {
            self.entries.insert(i, (id, name.to_string()));
        }
    }

    /// Record `face` and hand back what a glyph carries for it. A generic needs
    /// no entry — it names itself.
    pub(crate) fn intern(&mut self, face: &FontFace) -> FaceRef {
        match face {
            FontFace::Generic(generic) => FaceRef::Generic(*generic),
            FontFace::Named(name) => {
                let id = FaceId::of(name);
                self.insert(id, name);
                FaceRef::Named(id)
            }
        }
    }

    /// The face `attrs` name, interned — the walker's door.
    pub(crate) fn face_from_attrs(
        &mut self,
        attrs: &[(String, Option<String>)],
    ) -> Option<FaceRef> {
        FontFace::from_attrs(attrs).map(|face| self.intern(&face))
    }

    /// Take in everything `other` knows — how a build assembles one table out
    /// of the per-block walks, cache hits and spliced remnants it is made of.
    pub(crate) fn merge(&mut self, other: &FaceTable) {
        for (id, name) in &other.entries {
            self.insert(*id, name);
        }
    }
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
    /// How large this run is set — the author's `data-size` as a step or a
    /// point size, and `None` for the theme's own size, which is every glyph
    /// there was before the presentation vocabulary.
    ///
    /// Read at both levels, the nearer winning: a span's `data-size` inside a
    /// block carrying its own applies to the span. A frontend that ignores it
    /// draws one size, as `leaf-ratatui` does — a cell has one size.
    pub size: Option<FontSize>,
    /// The face this run is set in — the author's `data-font` as a generic or
    /// as an id into the map's [`FaceTable`], and `None` for the theme's body
    /// face. Read at the same two levels [`size`](Self::size) is.
    pub font: Option<FaceRef>,
    /// The run's *foreground* colour — the author's `data-color` on an
    /// attributed span, and `None` for the theme's text colour.
    ///
    /// The same seven names [`Role::Mark`] carries, over the same enum: a
    /// frontend that has a red for a highlight has a red for text, and both
    /// should be *that* red. The two never collide, because a `mark` is a
    /// `mark` and a span is a span — a `data-color` on a `mark` node is the
    /// highlight's background and reaches a glyph through its role, while this
    /// is what a `<span data-color="red">` paints the letters. Only this one
    /// opens to a triple, for the reason [`MarkColor`]'s note gives.
    pub color: Option<TextColor>,
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

    pub const fn size(mut self, s: Option<FontSize>) -> Self {
        self.size = s;
        self
    }

    pub const fn font(mut self, f: Option<FaceRef>) -> Self {
        self.font = f;
        self
    }

    pub const fn color(mut self, c: Option<TextColor>) -> Self {
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
        // A generic is a CSS keyword, and CSS reads a keyword either way — the
        // spelling written back is still the lowercase one.
        assert_eq!(FontFamily::from_attr("Serif"), Some(FontFamily::Serif));
        assert_eq!(
            FontFamily::from_attr("SANS-SERIF"),
            Some(FontFamily::SansSerif)
        );
        assert_eq!(
            FontFamily::from_attr("MonoSpace").unwrap().name(),
            "monospace"
        );
        assert_eq!(
            FontFace::from_attr("Serif"),
            Some(FontFace::Generic(FontFamily::Serif)),
            "and the open type reads it as the generic, not as a family name"
        );
        assert_eq!(FontFace::from_attr("Serif").unwrap().name(), "serif");
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

    /// Each open type reads the **name first** and the value second, and spells
    /// back what it read in one canonical form — so a size set twice from the
    /// same field writes the same bytes both times, and a document rewritten by
    /// leaf is a document a stylesheet can still key on where a name was used.
    #[test]
    fn each_property_reads_a_name_or_a_value_and_spells_one_of_them_back() {
        // Size: the seven keywords, then `<number>pt`.
        let size = |v: &str| FontSize::from_attr(v);
        assert_eq!(size("large"), Some(FontSize::Step(SizeStep::Large)));
        assert_eq!(size("14pt"), FontSize::points(14.0));
        assert_eq!(size("14.0pt"), size("14pt"), "the same size, spelled twice");
        assert_eq!(size(" 13.5pt "), FontSize::points(13.5));
        assert_eq!(size("14PT"), size("14pt"), "CSS reads its units either way");
        assert_eq!(size("14pt").unwrap().name(), "14pt");
        assert_eq!(size("13.50pt").unwrap().name(), "13.5pt");
        assert_eq!(size("13.25pt").unwrap().name(), "13.25pt");
        assert_eq!(size("large").unwrap().name(), "large");
        assert_eq!(size("14pt").unwrap().points_value(), Some(14.0));
        assert_eq!(size("large").unwrap().points_value(), None);
        assert_eq!(size("large").unwrap().step(), Some(SizeStep::Large));

        // Line height: the three names, then any positive decimal — and a
        // decimal that spells a name *is* the name.
        let lh = |v: &str| LineHeight::from_attr(v);
        assert_eq!(lh("1.5"), Some(LineHeight::Step(LineSpacing::OneHalf)));
        assert_eq!(lh("1.50"), lh("1.5"), "one spacing, not two");
        assert_eq!(lh("2.0"), Some(LineHeight::Step(LineSpacing::Double)));
        assert_eq!(lh("1.3"), LineHeight::ratio(1.3));
        assert_eq!(lh("1.3").unwrap().name(), "1.3");
        assert_eq!(lh("1.25").unwrap().name(), "1.25");
        assert_eq!(lh("1.5").unwrap().name(), "1.5");
        assert!((lh("1.3").unwrap().as_f32() - 1.3).abs() < 1e-6);
        assert!((lh("1.5").unwrap().as_f32() - 1.5).abs() < 1e-6);
        // Single is absence, however it is spelled, so a gesture given it
        // clears the key rather than writing a `data-line-height="1"` that
        // says nothing.
        for v in ["1", "1.0", "1.00", " 1 "] {
            assert_eq!(lh(v), None, "{v:?} is single, which is absence");
        }
        assert_eq!(LineHeight::ratio(1.0), None);

        // Colour: the seven names, `#rrggbb`, and `#rgb` read but never
        // written.
        let col = |v: &str| TextColor::from_attr(v);
        assert_eq!(col("red"), Some(TextColor::Named(MarkColor::Red)));
        assert_eq!(
            col("#c03030"),
            Some(TextColor::Rgb {
                r: 0xc0,
                g: 0x30,
                b: 0x30
            })
        );
        assert_eq!(col("#C03030"), col("#c03030"));
        assert_eq!(col("#f00"), col("#ff0000"), "each digit doubled");
        assert_eq!(col("#c03030").unwrap().name(), "#c03030");
        assert_eq!(col("red").unwrap().name(), "red");
        assert_eq!(col("#c03030").unwrap().rgb(), Some((0xc0, 0x30, 0x30)));
        assert_eq!(col("red").unwrap().named(), Some(MarkColor::Red));

        // Face: the four generics, then any other non-empty string.
        let face = |v: &str| FontFace::from_attr(v);
        assert_eq!(face("serif"), Some(FontFace::Generic(FontFamily::Serif)));
        assert_eq!(face("Garamond"), Some(FontFace::Named("Garamond".into())));
        assert_eq!(face("  Garamond  "), face("Garamond"));
        assert_eq!(face("Garamond").unwrap().name(), "Garamond");
        assert_eq!(face("serif").unwrap().name(), "serif");
        assert_eq!(face("Garamond").unwrap().generic(), None);
    }

    /// A value outside the grammar is what it was before the vocabulary opened:
    /// `None` here, carried untouched by the document, and drawn at the theme's
    /// default. `px` is a screen's unit and a document is not a screen; the
    /// relative units are what the steps already are; and a colour function is
    /// a CSS parser leaf is not going to become.
    #[test]
    fn a_value_outside_the_grammar_is_carried_and_not_guessed_at() {
        for v in ["huge", "14px", "1.3em", "14", "pt", "-14pt", "0pt", ""] {
            assert_eq!(FontSize::from_attr(v), None, "{v:?} is not a size");
        }
        for v in ["1.3em", "normal", "-1.3", "0", "1.2.3", ""] {
            assert_eq!(LineHeight::from_attr(v), None, "{v:?} is not a spacing");
        }
        for v in [
            "rgb(192, 48, 48)",
            "chartreuse",
            "#c0303",
            "#gggggg",
            "c03030",
            "#",
            "",
        ] {
            assert_eq!(TextColor::from_attr(v), None, "{v:?} is not a colour");
        }
        // A face has almost no grammar to fall outside of — only emptiness,
        // because a family name is whatever the author typed.
        assert_eq!(FontFace::from_attr("   "), None);
        assert_eq!(FontFace::from_attr(""), None);
        // And a bare attribute has no value at all, at every key.
        let bare = |k: &str| vec![(k.to_string(), None)];
        assert_eq!(FontSize::from_attrs(&bare("data-size")), None);
        assert_eq!(LineHeight::from_attrs(&bare("data-line-height")), None);
        assert_eq!(TextColor::from_attrs(&bare("data-color")), None);
        assert_eq!(FontFace::from_attrs(&bare("data-font")), None);
    }

    /// The fixed point is the reason a [`Style`] stays `Copy` and `Eq`, and
    /// hundredths is where the rounding lands. Pinned because the spelling is
    /// what a document is written with: a shortest decimal, so a size set twice
    /// from the same field writes the same bytes.
    #[test]
    fn a_value_is_hundredths_and_spells_itself_as_short_as_it_can() {
        let h = |v: f32| Hundredths::from_f32(v).unwrap().to_string();
        assert_eq!(h(14.0), "14");
        assert_eq!(h(13.5), "13.5");
        assert_eq!(h(1.25), "1.25");
        assert_eq!(h(1.3), "1.3");
        assert_eq!(h(0.05), "0.05");
        assert_eq!(Hundredths::from_f32(14.0).unwrap().as_f32(), 14.0);
        assert_eq!(Hundredths::from_f32(14.0).unwrap().hundredths(), 1400);
        // Out of what a u16 of hundredths holds, and out of what a size means.
        assert_eq!(Hundredths::from_f32(700.0), None);
        assert_eq!(Hundredths::from_f32(0.0), None);
        assert_eq!(Hundredths::from_f32(-1.0), None);
        assert_eq!(Hundredths::from_f32(f32::NAN), None);
        // And an integer part too long to hold is a `None`, not an overflow.
        assert_eq!(FontSize::from_attr("42949672.99pt"), None);
        assert_eq!(FontSize::from_attr("999999999999pt"), None);
        assert_eq!(FontSize::from_attr("655.36pt"), None);
        assert_eq!(FontSize::from_attr("655.35pt").unwrap().name(), "655.35pt");
        // A third decimal place rounds rather than being refused — a font panel
        // hands back whatever floating point made of its slider.
        assert_eq!(FontSize::from_attr("13.456pt"), FontSize::points(13.46));
        assert_eq!(FontSize::from_attr("13.454pt"), FontSize::points(13.45));
        // CSS wants digits on whichever side of the point the number has: `.5`
        // is a number, a trailing bare point is a typo.
        assert_eq!(FontSize::from_attr(".5pt"), FontSize::points(0.5));
        assert_eq!(LineHeight::from_attr(".5"), LineHeight::ratio(0.5));
        assert_eq!(FontSize::from_attr("14.pt"), None);
        assert_eq!(LineHeight::from_attr("1."), None);
        assert_eq!(FontSize::from_attr(".pt"), None);
    }

    /// A glyph carries a [`FaceId`], not a `String`, and the id is derived from
    /// the name so that a row built three different ways names one face. The
    /// table is the only place the string lives.
    #[test]
    fn a_named_face_is_interned_once_and_its_id_is_the_name_s_own() {
        let mut faces = FaceTable::default();
        let garamond = FontFace::Named("Garamond".into());
        let a = faces.intern(&garamond);
        let b = faces.intern(&FontFace::Named("Garamond".into()));
        assert_eq!(a, b, "one name, one id");
        assert_eq!(faces.len(), 1, "and one entry");
        assert_eq!(a, FaceRef::Named(FaceId::of("Garamond")));
        assert_eq!(faces.name(FaceId::of("Garamond")), Some("Garamond"));
        assert_eq!(a.generic(), None);

        // A generic names itself and needs no entry.
        let serif = faces.intern(&FontFace::Generic(FontFamily::Serif));
        assert_eq!(serif, FaceRef::Generic(FontFamily::Serif));
        assert_eq!(serif.id(), None);
        assert_eq!(faces.len(), 1);

        // Two tables that met the same names in opposite orders are equal, so
        // a map assembled out of cache hits compares against a fresh build.
        let mut one = FaceTable::default();
        one.intern(&FontFace::Named("Futura".into()));
        one.intern(&garamond);
        let mut two = FaceTable::default();
        two.intern(&garamond);
        two.intern(&FontFace::Named("Futura".into()));
        assert_eq!(one, two);

        // And a merge is how a build makes one table out of several walks.
        let mut merged = FaceTable::default();
        merged.merge(&one);
        merged.merge(&faces);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.name(FaceId::of("Futura")), Some("Futura"));
        assert_eq!(merged.name(FaceId::of("Bodoni")), None);
    }
}
