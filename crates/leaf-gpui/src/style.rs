//! The GUI end of leaf-core's styling seam: map the toolkit-neutral
//! [`leaf_core::Style`] — a [`Role`] plus emphasis flags — onto gpui's `Hsla`,
//! `Font`, and `TextRun`.
//!
//! This is the gpui counterpart of `leaf-tui`'s `to_ratatui`. Core hands over
//! *what a glyph is*, never what color to paint it; here the GUI answers with
//! its own palette and font metrics. Where a terminal tells a heading apart by
//! cycling a color, the GUI gives it a larger font in the ordinary text color —
//! and code a monospace family rather than a green tint — so the same neutral
//! roles produce a native-feeling document rather than a colored-terminal echo.

use gpui::{Font, FontStyle, FontWeight, Hsla, TextRun, UnderlineStyle, px};
use leaf_core::style::{Role, Style as LStyle};

/// The GUI's presentation of a glyph: the two font families it shapes with plus
/// the handful of role colors, resolved once per paint from the theme and handed
/// to the shaper (a run picks its family and color from its glyph's [`Role`]).
#[derive(Clone)]
pub struct RunStyle {
    /// Ordinary prose (and headings, which differ only in size and weight).
    pub body: Font,
    /// Code — a monospace family so columns line up.
    pub mono: Font,
    /// The default glyph color: body text, headings, and code all read in it.
    pub text: Hsla,
    /// A hyperlink's color.
    pub link: Hsla,
    /// Quiet decoration — list bullets, quote/code gutters, rules.
    pub muted: Hsla,
    /// The highlight behind marked (`==mark==`) text.
    pub mark_bg: Hsla,
    /// The tint behind an inline `` `code` `` run — the pill that sets it apart
    /// mid-sentence. A fenced block gets a drawn border box instead (geometry the
    /// element paints), but an inline run can only carry a background color.
    pub code_bg: Hsla,
}

/// How much larger than the body a heading of `level` (1-based) is drawn, given
/// the theme's per-level ramp. Levels past the ramp (there is no heading past 6)
/// fall back to body size.
pub fn heading_scale(level: u8, ramp: &[f32; 6]) -> f32 {
    level
        .checked_sub(1)
        .and_then(|i| ramp.get(i as usize))
        .copied()
        .unwrap_or(1.0)
}

/// A `TextRun` of `len` bytes for a glyph of style `s`: its [`Role`] picks the
/// family (mono for code), the color, and any background/underline; the author's
/// own bold/italic layer on top.
///
/// A run carries a `Font` but not a size — a line is shaped at one size (see
/// [`crate::Shaper`]), which is why a heading, whose whole row is bigger, is a
/// row-level decision while the family here is per-run. Headings are kept
/// *orthogonal* to color: they read in the default text color and are bold from
/// their role, so size and weight do all the distinguishing.
pub fn text_run(len: usize, s: LStyle, rs: &RunStyle) -> TextRun {
    let heading = matches!(s.role, Role::Heading(_));
    let mut font = match s.role {
        Role::Code => rs.mono.clone(),
        _ => rs.body.clone(),
    };
    // A heading is bold from its role even though the author wrote no emphasis;
    // every other role bolds only if he did.
    if s.bold || heading {
        font.weight = FontWeight::BOLD;
    }
    if s.italic {
        font.style = FontStyle::Italic;
    }
    let color = match s.role {
        Role::Link => rs.link,
        Role::ListMarker | Role::QuoteGutter | Role::Rule => rs.muted,
        // Body, Heading, Code, Mark all read in the default text color.
        _ => rs.text,
    };
    // Marked text gets its highlight; inline code gets its pill tint. A fenced
    // code block's rows get the same tint from a quad the element paints behind
    // them, so the per-run background here reads the same either way.
    let background_color = match s.role {
        Role::Mark => Some(rs.mark_bg),
        Role::Code => Some(rs.code_bg),
        _ => None,
    };
    // Links are underlined; so is anything the author marked as an insertion
    // (`{+ins+}` sets the underline flag). `color: None` follows the run's color.
    let underline = (matches!(s.role, Role::Link) || s.underline)
        .then(|| UnderlineStyle { thickness: px(1.0), color: None, wavy: false });
    TextRun {
        len,
        font,
        color,
        background_color,
        underline,
        strikethrough: None,
    }
}
