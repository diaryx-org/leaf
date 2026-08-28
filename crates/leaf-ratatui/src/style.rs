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

use leaf_core::VisualMap;
use leaf_core::style::{Role, Style as LStyle};
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
    theme: &Theme,
    code_shift: impl Fn(usize) -> Option<usize>,
) -> Vec<Line<'static>> {
    let (ss, se) = sel.unwrap_or((usize::MAX, usize::MAX));
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
                let mut style = theme.to_ratatui(g.style);
                if g.src >= ss && g.src < se {
                    style = style.add_modifier(Modifier::REVERSED);
                }
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
