//! The GUI end of leaf-core's styling seam: map the toolkit-neutral
//! [`leaf_core::Style`]/[`leaf_core::Color`]/[`leaf_core::style::Role`] onto
//! gpui's `Hsla` + `TextRun`.
//!
//! This is the gpui counterpart of `leaf-tui`'s `to_ratatui`. Same neutral
//! `Style` coming out of `leaf-core`'s WYSIWYG builder; here a GUI is free to
//! give each `Color` role a real RGB value, and each typographic `Role` a real
//! font family and size, instead of a terminal slot.

use gpui::{Font, FontStyle, FontWeight, Hsla, TextRun, rgb};
use leaf_core::style::{Color as LColor, Role, Style as LStyle};

/// The two font families the widget shapes with: `body` for ordinary text (and
/// headings, which differ from it only in size and weight), `mono` for code.
/// Both are resolved once per paint from the theme and handed to the shaper,
/// since a run picks its family from the glyph's [`Role`].
#[derive(Clone)]
pub struct Fonts {
    pub body: Font,
    pub mono: Font,
}

/// Give each neutral color role a concrete RGB. A theme would drive these; the
/// scaffold hard-codes a readable light-background palette.
pub fn to_hsla(c: LColor) -> Hsla {
    let hex: u32 = match c {
        LColor::Default => 0x1e1e1e,
        LColor::Black => 0x000000,
        LColor::Cyan => 0x0a7ea4,
        LColor::Green => 0x2e8b57,
        LColor::Yellow => 0xb8860b,
        LColor::Blue => 0x1e66f5,
        LColor::Magenta => 0x8b1a89,
        LColor::Gray => 0x707070,
        LColor::DarkGray => 0x9a9a9a,
    };
    rgb(hex).into()
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

/// A `TextRun` of `len` bytes carrying a neutral style: the family its [`Role`]
/// asks for (mono for code), the color its [`Color`] asks for, and bold/italic
/// via font weight/slant.
///
/// A run carries a `Font` but not a size — a line is shaped at one size (see
/// [`crate::Shaper`]), which is why a heading, whose whole row is bigger, is a
/// row-level decision while the family here is per-run. Headings are kept
/// *orthogonal* to color: they read in the default text color regardless of the
/// hue the terminal uses to tell levels apart, so size and weight do all the
/// distinguishing.
pub fn text_run(len: usize, s: LStyle, fonts: &Fonts) -> TextRun {
    let heading = matches!(s.role, Role::Heading(_));
    let mut font = match s.role {
        Role::Code => fonts.mono.clone(),
        _ => fonts.body.clone(),
    };
    if s.bold {
        font.weight = FontWeight::BOLD;
    }
    if s.italic {
        font.style = FontStyle::Italic;
    }
    let fg = if heading { LColor::Default } else { s.fg };
    TextRun {
        len,
        font,
        color: to_hsla(fg),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}
