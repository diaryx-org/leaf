//! The terminal end of the styling seam: map leaf-core's toolkit-neutral
//! [`leaf_core::Style`] onto a `ratatui::Style`, and turn a WYSIWYG
//! [`VisualMap`] into styled ratatui lines.
//!
//! This is the code that used to live on `VisualMap::to_lines` in the old
//! single-crate leaf. It moved here because a `Line`/`Span` is a ratatui type;
//! the geometry it reads (glyphs, source offsets) stays in leaf-core so a GUI
//! frontend reuses it unchanged.
//!
//! The palette itself is a *value* — a [`Theme`] — not a set of constants. A
//! terminal's own background is whatever the user themed it, so a fixed dark
//! grey behind code is a bug on a light terminal, not a style choice: the
//! colors have to be picked against the surface they land on. [`Theme::dark`]
//! and [`Theme::light`] are the two curated palettes, [`detect_color_scheme`]
//! (plus [`crate::EditorState::query_color_scheme`]) works out which one the
//! terminal wants, and a host that disagrees can hand over its own.
//!
//! This mirrors `leaf-gpui`'s `RunStyle`, which is likewise a palette passed in
//! per paint rather than baked into the mapping function.

use leaf_core::style::{Role, Style as LStyle};
use leaf_core::{Highlight, HighlightCursor, VisualMap};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub use leaf_core::ColorScheme;

/// How far a fenced code block's text is inset from the left edge of its box —
/// one column, the room the box's left border sits in. The caret and mouse math
/// in `ui` shift a code row's columns by this same amount.
pub const CODE_INSET: usize = 1;

/// The environment variable that pins the color scheme, skipping detection
/// entirely: `light` or `dark` (case-insensitive), anything else ignored. The
/// escape hatch for a terminal that answers neither the `OSC 11` query nor
/// `COLORFGBG` — over a bare SSH session, say — and gets guessed wrong.
pub const THEME_ENV: &str = "LEAF_THEME";

/// The terminal's palette, keyed on a glyph's semantic [`Role`]. This is the
/// presentation the core used to bake in and no longer does: a terminal can
/// only tell a heading from body text by *color*, so the choice of which color
/// lives here, in the frontend that has the constraint — not in the shared
/// model. A GUI, which can vary size and font, maps the same roles to entirely
/// different looks.
///
/// Two curated variants ship: [`Theme::dark`] and [`Theme::light`]. They differ
/// in more than the backgrounds — a hue that reads on black often doesn't read
/// on white (ANSI yellow on a light terminal is the worst offender), so the
/// light palette darkens the whole ramp rather than only re-tinting the panels.
/// Every field is public, so a host that wants a third look can start from one
/// of these and override what it likes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Theme {
    /// Which curated palette this is — carried so [`crate::EditorState`] can
    /// answer "light or dark?" for `<picture>` resolution without a second
    /// source of truth.
    pub scheme: ColorScheme,
    /// Heading colors by level, `[0]` = level 1. Levels past 6 reuse the last.
    /// All are drawn bold, and level 1 additionally underlined.
    pub heading: [Color; 6],
    /// Code text — inline `` `code` `` and the contents of a fenced block.
    pub code_fg: Color,
    /// The tint behind code: the pill behind an inline run and the fill of a
    /// fenced block's box. Close to the terminal's own background so the panel
    /// reads as a subtle raise rather than a slab.
    pub code_bg: Color,
    /// A fenced block box's border.
    pub code_border: Color,
    /// The language label riding a fenced block's top border.
    pub code_label: Color,
    /// A hyperlink's visible text (drawn underlined).
    pub link: Color,
    /// Marked (`==mark==`) text: dark ink on a highlighter wash.
    pub mark_fg: Color,
    /// The highlighter wash behind marked text.
    pub mark_bg: Color,
    /// A list item's bullet or number.
    pub list_marker: Color,
    /// A block quote's `│` gutter.
    pub quote_gutter: Color,
    /// Thematic breaks and table rules.
    pub rule: Color,
    /// Raw markup revealed on the caret's line (`MarkupMode::Full`).
    pub delimiter: Color,
    /// A block image's `🖼 alt` placeholder text.
    pub image: Color,
    /// The wash behind a host-painted range — [`leaf_core::Highlight`] — that
    /// names no color of its own. Search hits are the obvious one; an
    /// annotation layer is the other.
    ///
    /// Its own pair rather than a reuse of [`mark_bg`](Self::mark_bg), because
    /// a `==mark==` is *in* the document and a highlight is painted over it,
    /// and a reader has to be able to tell "the author emphasised this" from
    /// "your search found this". A wash rather than a reverse, so the selection
    /// — which does reverse — still reads as distinct on top of it, which is
    /// what makes the current search hit stand out from the rest.
    pub highlight_bg: Color,
    /// The ink on [`highlight_bg`](Self::highlight_bg).
    pub highlight_fg: Color,
    /// The fill behind a floating host overlay — the context menu, the command
    /// palette, the key reference, the dialogs, the status toast.
    ///
    /// The chrome colors below exist for the same reason the rest of this
    /// palette does, and were the last place in leaf still ignoring it: a
    /// hardcoded dark-grey panel with white text is a dark-mode assumption, and
    /// on a light terminal it renders a menu as a black hole with grey text in
    /// it. A panel has to be a legible step off *the user's* page in whichever
    /// direction their page runs.
    pub panel_bg: Color,
    /// An overlay's ordinary text.
    pub panel_fg: Color,
    /// An overlay's quiet text: section headers, key hints in the footer, and
    /// the rows a document's format can't run. Muted enough to recede, not so
    /// muted it stops being readable — a disabled row still has to be legible,
    /// because saying what a format *can't* do is the whole reason it's drawn.
    pub panel_dim: Color,
    /// The accent on an overlay: a key chord, a checked row, the palette's
    /// prompt. Kin to [`link`](Self::link), which is the same "this is
    /// actionable" signal in the body.
    pub panel_accent: Color,
    /// The highlighted row's fill. An explicit color rather than `REVERSED`,
    /// which inverts against the *panel* and so lands somewhere different in
    /// each scheme — and, on a checked row, throws away the accent it was
    /// carrying.
    pub panel_selected_bg: Color,
    /// The highlighted row's text.
    pub panel_selected_fg: Color,
    /// What's at stake in a dialog, and the status toast. ANSI yellow on a light
    /// terminal is the worst offender in the whole palette, so the light scheme
    /// takes amber here exactly as it does for headings.
    pub panel_warning: Color,
    /// The frame drawn around a block image's reserved area — the picture sits
    /// inside it, and it stands alone as the "picture goes here" placeholder
    /// when the raster can't be painted (a remote/unresolved image, or one
    /// scrolled so it doesn't fully fit — a graphics-protocol image can't be
    /// clipped, but this cell-drawn border can). A muted magenta, kin to the
    /// `🖼` [`Role::Image`] label.
    pub image_border: Color,
}

impl Default for Theme {
    /// The dark palette — the safe guess for a terminal that hasn't been asked
    /// yet, since the large majority of terminals ship dark.
    fn default() -> Self {
        Theme::dark()
    }
}

impl Theme {
    /// The palette for a dark terminal: saturated ANSI hues, which the user's
    /// own terminal theme has already tuned to read on their background, and
    /// quiet 256-color greys for the code panel.
    pub const fn dark() -> Self {
        Theme {
            scheme: ColorScheme::Dark,
            heading: [
                Color::Cyan,
                Color::Green,
                Color::Yellow,
                Color::Blue,
                Color::Magenta,
                Color::Gray,
            ],
            code_fg: Color::Green,
            code_bg: Color::Indexed(235),
            code_border: Color::Indexed(240),
            code_label: Color::Gray,
            link: Color::Cyan,
            mark_fg: Color::Black,
            mark_bg: Color::Yellow,
            list_marker: Color::Yellow,
            quote_gutter: Color::Green,
            rule: Color::DarkGray,
            delimiter: Color::DarkGray,
            image: Color::Magenta,
            image_border: Color::Indexed(96),
            // A blue wash, a step off the page rather than a slab, and well
            // clear of the yellow `mark_bg` above it.
            highlight_bg: Color::Indexed(24),
            highlight_fg: Color::Indexed(255),
            // A shade above `code_bg` (235), so an overlay floating over a code
            // block still reads as being *over* it rather than part of it.
            panel_bg: Color::Indexed(238),
            panel_fg: Color::Indexed(253),
            // 249, not the 247 that looks right in isolation: dim text on a
            // *panel* has a lighter ground under it than dim text on the page,
            // so it has to be lighter still to keep the same ~4.5:1 separation.
            panel_dim: Color::Indexed(249),
            panel_accent: Color::Cyan,
            panel_selected_bg: Color::Indexed(24),
            panel_selected_fg: Color::Indexed(255),
            panel_warning: Color::Yellow,
        }
    }

    /// The palette for a light terminal. The panels invert — a near-white grey
    /// behind code instead of a near-black one — and the hues are named as
    /// explicit dark 256-color indices rather than ANSI names. That last part
    /// is the whole point: ANSI yellow, green, and cyan are chosen by terminal
    /// themes to sit on a *dark* background, and on white they wash out to
    /// unreadable. Amber stands in for yellow, and every hue is taken from the
    /// dark half of the color cube.
    ///
    /// The indices aren't eyeballed. Each is the one whose contrast against a
    /// white page comes nearest its dark-palette counterpart's contrast against
    /// a black one, so the two themes carry the same *weight* — the code fill is
    /// as slight a lift off the page (1.40 vs the dark's 1.39), the box border
    /// as faint against that fill (2.17 vs 2.13), the rules as quiet. Where the
    /// cube can't reach that far the light side lands short rather than
    /// overshooting into a color that shouts: there is no cyan as strong on
    /// white as ANSI cyan is on black.
    pub const fn light() -> Self {
        Theme {
            scheme: ColorScheme::Light,
            heading: [
                Color::Indexed(23),  // dark cyan
                Color::Indexed(22),  // dark green
                Color::Indexed(94),  // amber — ANSI yellow is invisible on white
                Color::Indexed(26),  // blue
                Color::Indexed(90),  // dark magenta
                Color::Indexed(240), // grey
            ],
            code_fg: Color::Indexed(22),
            code_bg: Color::Indexed(253),
            code_border: Color::Indexed(246),
            code_label: Color::Indexed(240),
            link: Color::Indexed(26),
            mark_fg: Color::Black,
            mark_bg: Color::Indexed(220),
            list_marker: Color::Indexed(94),
            quote_gutter: Color::Indexed(22),
            rule: Color::Indexed(243),
            delimiter: Color::Indexed(243),
            image: Color::Indexed(96),
            image_border: Color::Indexed(96),
            // The light half of the same idea: a pale blue tint the page can
            // carry, with the page's own dark ink left on top of it.
            highlight_bg: Color::Indexed(153),
            highlight_fg: Color::Indexed(234),
            // A shade *below* `code_bg` (253) for the same reason the dark panel
            // is a shade above its own: the overlay floats over the page, so it
            // steps away from it, and the step runs downward on a light one.
            panel_bg: Color::Indexed(251),
            panel_fg: Color::Indexed(234),
            panel_dim: Color::Indexed(239),
            // A shade darker than `link`'s 26: a key chord sits on the panel's
            // grey rather than on the page's white, and loses contrast to it.
            panel_accent: Color::Indexed(25),
            panel_selected_bg: Color::Indexed(24),
            panel_selected_fg: Color::Indexed(255),
            // The same amber the light palette gives headings and list markers,
            // rather than a red that would read better and belong to nothing:
            // ANSI yellow is invisible here, and this is where the light theme
            // already decided what stands in for it.
            panel_warning: Color::Indexed(94),
        }
    }

    /// The curated palette for a scheme.
    pub const fn for_scheme(scheme: ColorScheme) -> Self {
        match scheme {
            ColorScheme::Dark => Theme::dark(),
            ColorScheme::Light => Theme::light(),
        }
    }

    /// The base style for a glyph's role; the caller layers the author's own
    /// emphasis (bold/italic/…) on top. Headings cycle a color by level and are
    /// bold, exactly as before the palette became a value.
    fn role_style(&self, role: Role) -> Style {
        let s = Style::default();
        match role {
            Role::Body => s,
            Role::Heading(level) => {
                let idx = (level.max(1) as usize - 1).min(self.heading.len() - 1);
                let base = s.fg(self.heading[idx]).add_modifier(Modifier::BOLD);
                if level <= 1 {
                    base.add_modifier(Modifier::UNDERLINED)
                } else {
                    base
                }
            }
            // Code reads in its own hue on the code tint — inline it's the whole
            // pill, in a fenced block the box's fill matches so the two blend.
            Role::Code => s.fg(self.code_fg).bg(self.code_bg),
            Role::Link => s.fg(self.link).add_modifier(Modifier::UNDERLINED),
            Role::Mark => s.fg(self.mark_fg).bg(self.mark_bg),
            Role::ListMarker => s.fg(self.list_marker),
            Role::QuoteGutter => s.fg(self.quote_gutter),
            // Thematic breaks and table rules are quiet grey.
            Role::Rule => s.fg(self.rule),
            // Raw markup on the revealed line (`MarkupMode::Full`). Dim grey, the
            // same quiet the rules get: the delimiters are scaffolding around the
            // prose, and the line should still read as a line of text rather than
            // as a line of source. It sits *under* the author's own emphasis, which
            // the caller layers on after — so the `*` around a bold run comes out
            // dim and bold, which is what marks it as that run's delimiter.
            Role::Delimiter => s.fg(self.delimiter),
            // A block image's `🖼 alt` placeholder: the terminal has no raster
            // primitive, so it paints the label — dim magenta to read as a
            // stand-in for content it can't draw, not as prose.
            Role::Image => s.fg(self.image).add_modifier(Modifier::DIM),
        }
    }

    /// Map a neutral core style onto a ratatui style: the role picks the
    /// palette, then the author's own emphasis flags layer on top.
    pub fn to_ratatui(&self, s: LStyle) -> Style {
        let mut out = self.role_style(s.role);
        if s.bold {
            out = out.add_modifier(Modifier::BOLD);
        }
        if s.italic {
            out = out.add_modifier(Modifier::ITALIC);
        }
        if s.underline {
            out = out.add_modifier(Modifier::UNDERLINED);
        }
        if s.strikethrough {
            out = out.add_modifier(Modifier::CROSSED_OUT);
        }
        out
    }
}

/// The color scheme worked out from the environment alone — no terminal I/O, so
/// this is safe to call before the terminal is in raw mode (and it's what
/// [`Theme::default`] stands in for until a real query runs).
///
/// [`THEME_ENV`] wins outright; failing that, `COLORFGBG` (set by rxvt, konsole,
/// and a handful of others) names the background's ANSI index. Neither present,
/// the answer is [`ColorScheme::Dark`] — the majority default, and the one this
/// editor looked like before it could ask at all.
///
/// For the accurate answer — an `OSC 11` query to the terminal itself — see
/// [`crate::EditorState::query_color_scheme`].
pub fn detect_color_scheme() -> ColorScheme {
    scheme_from_env().unwrap_or(ColorScheme::Dark)
}

/// [`detect_color_scheme`] without the fallback: `None` means the environment
/// said nothing, which is the cue to ask the terminal directly.
pub(crate) fn scheme_from_env() -> Option<ColorScheme> {
    scheme_from_name(&std::env::var(THEME_ENV).ok()?)
        .or_else(|| scheme_from_colorfgbg(&std::env::var("COLORFGBG").ok()?))
}

/// Parse a [`THEME_ENV`] value: `light` or `dark`, case- and space-insensitive.
/// `None` for anything else — including `auto`, which is how you ask for
/// detection back.
fn scheme_from_name(value: &str) -> Option<ColorScheme> {
    match value.trim().to_ascii_lowercase().as_str() {
        "light" => Some(ColorScheme::Light),
        "dark" => Some(ColorScheme::Dark),
        _ => None,
    }
}

/// Parse a `COLORFGBG` value (`"fg;bg"` or `"fg;;bg"`) into a scheme: the
/// trailing background ANSI index is dark for 0–6 and 8, light otherwise.
/// `None` when there's no parseable trailing index.
fn scheme_from_colorfgbg(value: &str) -> Option<ColorScheme> {
    let bg: u8 = value.rsplit(';').next()?.trim().parse().ok()?;
    Some(if bg <= 6 || bg == 8 {
        ColorScheme::Dark
    } else {
        ColorScheme::Light
    })
}

/// Styled ratatui lines for the WYSIWYG map, drawing any glyph whose source
/// offset is within the `[start, end)` selection reversed. Adjacent glyphs of
/// equal style are merged into one span.
///
/// `code_shift(row)` returns `Some(scroll)` for a fenced code-block row — the
/// display columns to scroll it left inside its box — and `None` for ordinary
/// text. A code row is drawn inset by [`CODE_INSET`] (room for the box's left
/// border) and scrolled: the leading `scroll` columns are dropped so a long line
/// slides under the box rather than wrapping or running off the right edge. The
/// caret and mouse in `ui` mirror this exact shift, so a code column still round-
/// trips to its source byte.
pub fn wysiwyg_lines(
    vmap: &VisualMap,
    sel: Option<(usize, usize)>,
    highlights: &[Highlight],
    theme: &Theme,
    code_shift: impl Fn(usize) -> Option<usize>,
) -> Vec<Line<'static>> {
    // One cursor for the whole view: the rows are walked in order and so are
    // their glyphs, which is exactly what it is for.
    let mut covering = HighlightCursor::new(highlights);
    vmap.rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            let shift = code_shift(r);
            let mut spans: Vec<Span<'static>> = Vec::new();
            // A code row opens with its inset: the columns the box's left border
            // lands on, tinted so the fill runs edge to edge under the border.
            if shift.is_some() {
                spans.push(Span::styled(
                    " ".repeat(CODE_INSET),
                    Style::default().bg(theme.code_bg),
                ));
            }
            let mut buf = String::new();
            let mut cur: Option<Style> = None;
            // Column of the current glyph within the row, to drop the ones a code
            // row has scrolled off its left edge.
            let mut col = 0usize;
            for g in &row.glyphs {
                let w = char_cols(g.ch);
                let hidden = shift.is_some_and(|scroll| col < scroll);
                col += w;
                if hidden {
                    continue;
                }
                let style = composed(theme.to_ratatui(g.style), g.src, sel, &mut covering, theme);
                if cur == Some(style) {
                    buf.push(g.ch);
                } else {
                    if let Some(s) = cur.take() {
                        spans.push(Span::styled(std::mem::take(&mut buf), s));
                    }
                    cur = Some(style);
                    buf.push(g.ch);
                }
            }
            if let Some(s) = cur {
                spans.push(Span::styled(buf, s));
            }
            if spans.is_empty() {
                spans.push(Span::raw(""));
            }
            Line::from(spans)
        })
        .collect()
}

/// A glyph's or a run's final style at source byte `at`: its own style, the
/// host's wash under it, the selection reversed over that.
///
/// One function because both painters compose the same three layers and had
/// better not disagree about the order. A host paints a range to say "this is
/// one of the things you asked for"; the selection says "and this is the one
/// you are on", so the two compose rather than one replacing the other —
/// reversing the wash is what makes the current search hit legible among the
/// rest of them.
pub(crate) fn composed(
    base: Style,
    at: usize,
    sel: Option<(usize, usize)>,
    highlights: &mut HighlightCursor<'_>,
    theme: &Theme,
) -> Style {
    let mut style = base;
    if let Some(h) = highlights.at(at) {
        style = highlight_wash(style, h, theme);
    }
    if sel.is_some_and(|(s, e)| at >= s && at < e) {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

/// Lay a highlight's wash under a glyph's own style, keeping whatever the
/// author's markup put there (bold, italic, underline) and replacing only the
/// ink and the ground.
///
/// A `#RRGGBB` on the highlight overrides the theme's ground, since the point of
/// the field is a host that colors its own annotations — and then the ink has to
/// be picked against *that* ground rather than left as the theme's. The theme's
/// `highlight_fg` is chosen to read on the theme's own wash; forcing it onto a
/// color the host named puts the dark palette's near-white ink on `#ffe066`,
/// which is the failure this branch exists for. The two curated inks are the
/// candidates, and [`contrast_ink`] picks whichever the host's color can carry.
pub(crate) fn highlight_wash(style: Style, highlight: &Highlight, theme: &Theme) -> Style {
    match highlight.color.as_deref().and_then(parse_rgb) {
        Some((r, g, b)) => style.fg(contrast_ink(r, g, b)).bg(Color::Rgb(r, g, b)),
        None => style.fg(theme.highlight_fg).bg(theme.highlight_bg),
    }
}

/// The ink that reads on a host-supplied `#RRGGBB` wash: the light palette's
/// near-black on a light color, the dark palette's near-white on a dark one.
///
/// The two curated `highlight_fg`s rather than a computed shade, so a
/// host-colored range still looks like it belongs to this editor. The split is
/// by relative luminance, which is coarse next to a real contrast ratio and
/// exactly right for the one decision being made — the two candidates sit at
/// opposite ends of the scale, so anything nearer one than the other is a
/// comfortable choice rather than a marginal one.
fn contrast_ink(r: u8, g: u8, b: u8) -> Color {
    let luminance = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
    if luminance > 0.5 * 255.0 {
        Theme::light().highlight_fg
    } else {
        Theme::dark().highlight_fg
    }
}

/// `#RRGGBB` (or `RRGGBB`) as its three components; `None` for anything else,
/// which falls back to the theme rather than erroring — a malformed hint from a
/// host is not worth refusing to draw the document over.
///
/// The components rather than a `Color`, because the caller has to weigh them
/// to pick an ink that reads on them ([`contrast_ink`]).
fn parse_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// The display-column width of one glyph — the terminal's own measure, matched
/// to how `leaf-core` lays a row out into columns so a scrolled code column and
/// its caret can't drift apart.
fn char_cols(ch: char) -> usize {
    leaf_core::wysiwyg::text_width(ch.encode_utf8(&mut [0u8; 4]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorfgbg_reads_the_trailing_background_index() {
        // "light text on dark bg" (bg 0) → dark; "dark on light" (bg 15) → light.
        assert_eq!(scheme_from_colorfgbg("15;0"), Some(ColorScheme::Dark));
        assert_eq!(scheme_from_colorfgbg("0;15"), Some(ColorScheme::Light));
        // The three-field forms some terminals emit.
        assert_eq!(scheme_from_colorfgbg("15;;0"), Some(ColorScheme::Dark));
        assert_eq!(
            scheme_from_colorfgbg("15;default;0"),
            Some(ColorScheme::Dark)
        );
        // 8 is a dark grey background, not a light one.
        assert_eq!(scheme_from_colorfgbg("7;8"), Some(ColorScheme::Dark));
        assert_eq!(scheme_from_colorfgbg("7;7"), Some(ColorScheme::Light));
        assert_eq!(scheme_from_colorfgbg("7"), Some(ColorScheme::Light));
        assert_eq!(scheme_from_colorfgbg(""), None);
        assert_eq!(scheme_from_colorfgbg("nonsense"), None);
        assert_eq!(scheme_from_colorfgbg("default;default"), None);
    }

    #[test]
    fn theme_env_names_a_scheme_and_nothing_else() {
        assert_eq!(scheme_from_name("light"), Some(ColorScheme::Light));
        assert_eq!(scheme_from_name(" DARK "), Some(ColorScheme::Dark));
        // `auto` is how you ask for detection, so it must not resolve.
        assert_eq!(scheme_from_name("auto"), None);
        assert_eq!(scheme_from_name(""), None);
    }

    /// The `#RRGGBB` a host may put on a highlight, and the shapes that aren't
    /// one — a malformed hint falls back to the theme rather than refusing to
    /// draw the document.
    #[test]
    fn a_highlight_colour_is_read_as_hex_or_ignored() {
        assert_eq!(parse_rgb("#ffe066"), Some((0xff, 0xe0, 0x66)));
        assert_eq!(parse_rgb("ffe066"), Some((0xff, 0xe0, 0x66)));
        assert_eq!(parse_rgb("#fff"), None);
        assert_eq!(parse_rgb("rebeccapurple"), None);
        assert_eq!(parse_rgb("#gggggg"), None);
        assert_eq!(parse_rgb(""), None);
    }

    /// A host that colours its own range has to be given an ink that reads on
    /// it. The dark theme's near-white on `#ffe066` is the bug this is for, and
    /// the answer must not depend on which theme is in force: the wash is the
    /// host's colour in both, so the ink is too.
    #[test]
    fn a_host_coloured_highlight_gets_an_ink_that_reads_on_it() {
        let painted = |hex: &str| Highlight {
            start: 0,
            end: 1,
            id: String::new(),
            color: Some(hex.into()),
            marker: None,
        };
        let dark_ink = Theme::dark().highlight_fg;
        let light_ink = Theme::light().highlight_fg;
        for theme in [Theme::dark(), Theme::light()] {
            let wash = |hex: &str| highlight_wash(Style::default(), &painted(hex), &theme);
            // A pale wash takes the dark ink, whichever theme is running.
            assert_eq!(wash("#ffe066").fg, Some(light_ink), "pale yellow");
            assert_eq!(wash("#ffffff").fg, Some(light_ink), "white");
            // A dark one takes the light ink.
            assert_eq!(wash("#1a1a2e").fg, Some(dark_ink), "near-black navy");
            assert_eq!(wash("#000000").fg, Some(dark_ink), "black");
            // Either way the ground is what the host asked for.
            assert_eq!(wash("#ffe066").bg, Some(Color::Rgb(0xff, 0xe0, 0x66)));
            // And a highlight with no colour is still the theme's own pair.
            let plain = Highlight {
                color: None,
                ..painted("")
            };
            let style = highlight_wash(Style::default(), &plain, &theme);
            assert_eq!(style.bg, Some(theme.highlight_bg));
            assert_eq!(style.fg, Some(theme.highlight_fg));
        }
    }

    /// The rendering half of `Doc::set_highlights`, which no frontend painted
    /// before: a host-painted range gets the theme's wash, and only the glyphs
    /// inside it do.
    #[test]
    fn a_highlight_washes_only_the_glyphs_it_covers() {
        let mut doc =
            leaf_core::Doc::from_source("one two three\n".into(), leaf_core::Format::Markdown)
                .unwrap();
        doc.build_visual(40);
        let theme = Theme::dark();
        let painted = vec![Highlight {
            start: 4,
            end: 7,
            id: "hit".into(),
            color: None,
            marker: None,
        }];

        let lines = wysiwyg_lines(&doc.vmap, None, &painted, &theme, |_| None);
        let washed: String = lines[0]
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(theme.highlight_bg))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(washed, "two");
    }

    /// The current search hit is told apart from the rest by being the
    /// *selection*, so the two have to compose rather than one winning.
    #[test]
    fn the_selection_reverses_on_top_of_a_highlight_rather_than_replacing_it() {
        let mut doc =
            leaf_core::Doc::from_source("one two three\n".into(), leaf_core::Format::Markdown)
                .unwrap();
        doc.build_visual(40);
        let theme = Theme::dark();
        let painted = vec![Highlight {
            start: 4,
            end: 7,
            id: "hit".into(),
            color: None,
            marker: None,
        }];

        let lines = wysiwyg_lines(&doc.vmap, Some((4, 7)), &painted, &theme, |_| None);
        let span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "two")
            .expect("the selected match should still be one span");
        assert_eq!(span.style.bg, Some(theme.highlight_bg), "the wash survives");
        assert!(
            span.style.add_modifier.contains(Modifier::REVERSED),
            "and the selection reverses it"
        );
    }

    /// A host colour overrides the theme's ground; the ink stays the theme's,
    /// which is the only one leaf knows reads on anything.
    #[test]
    fn a_hosts_own_colour_replaces_the_wash_but_not_the_ink() {
        let theme = Theme::dark();
        let highlight = Highlight {
            start: 0,
            end: 1,
            id: "a".into(),
            color: Some("#102030".into()),
            marker: None,
        };
        let style = highlight_wash(Style::default(), &highlight, &theme);
        assert_eq!(style.bg, Some(Color::Rgb(0x10, 0x20, 0x30)));
        assert_eq!(style.fg, Some(theme.highlight_fg));
    }

    /// The bug this palette exists to fix: on a light terminal nothing may be
    /// painted with a near-black fill, and vice versa.
    #[test]
    fn each_palette_tints_code_toward_its_own_background() {
        let dark = match Theme::dark().code_bg {
            Color::Indexed(i) => i,
            other => panic!("expected an indexed grey, got {other:?}"),
        };
        let light = match Theme::light().code_bg {
            Color::Indexed(i) => i,
            other => panic!("expected an indexed grey, got {other:?}"),
        };
        // The 256-color greyscale ramp runs 232 (near-black) to 255 (near-white).
        assert!(
            (232..=243).contains(&dark),
            "dark code_bg {dark} isn't dark"
        );
        assert!(
            (244..=255).contains(&light),
            "light code_bg {light} isn't light"
        );
    }

    /// A light terminal must not be handed ANSI yellow/green/cyan, which
    /// terminal themes pick to read on black.
    #[test]
    fn the_light_palette_avoids_the_ansi_names() {
        let t = Theme::light();
        let named = [
            t.code_fg,
            t.link,
            t.list_marker,
            t.quote_gutter,
            t.rule,
            t.delimiter,
            t.image,
        ]
        .into_iter()
        .chain(t.heading)
        .filter(|c| !matches!(c, Color::Indexed(_)))
        .collect::<Vec<_>>();
        assert!(
            named.is_empty(),
            "light palette leans on ANSI names: {named:?}"
        );
    }

    #[test]
    fn a_scheme_picks_its_palette() {
        assert_eq!(Theme::for_scheme(ColorScheme::Light), Theme::light());
        assert_eq!(Theme::for_scheme(ColorScheme::Dark), Theme::dark());
        assert_eq!(Theme::default(), Theme::dark());
    }

    /// Headings past the ramp clamp to its last entry rather than panicking on
    /// an out-of-range index.
    #[test]
    fn heading_levels_clamp_to_the_ramp() {
        let t = Theme::dark();
        let color = |level| t.role_style(Role::Heading(level)).fg.unwrap();
        assert_eq!(color(1), t.heading[0]);
        assert_eq!(color(6), t.heading[5]);
        assert_eq!(color(9), t.heading[5]);
        // Level 0 is not a thing, but a malformed AST must not index out of range.
        assert_eq!(color(0), t.heading[0]);
    }

    /// Only level 1 is underlined — the rest are told apart by hue and weight.
    #[test]
    fn only_the_top_heading_is_underlined() {
        let t = Theme::dark();
        assert!(
            t.role_style(Role::Heading(1))
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(
            !t.role_style(Role::Heading(2))
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }
}
