//! The GUI end of leaf-core's styling seam: map the toolkit-neutral
//! [`leaf_core::Style`]/[`leaf_core::Color`] onto gpui's `Hsla` + `TextRun`.
//!
//! This is the gpui counterpart of `leaf-tui`'s `to_ratatui`. Same neutral
//! `Style` coming out of `leaf-core`'s WYSIWYG builder; here a GUI is free to
//! give each `Color` role a real RGB value instead of a terminal slot.

use gpui::{Font, FontStyle, FontWeight, Hsla, TextRun, rgb};
use leaf_core::style::{Color as LColor, Style as LStyle};

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

/// A `TextRun` of `len` bytes carrying a neutral style: foreground color plus
/// bold/italic via font weight/slant. (Underline/strikethrough/background are
/// left for a follow-up; the source view only needs the default run.)
pub fn text_run(len: usize, s: LStyle, base: &Font) -> TextRun {
    let mut font = base.clone();
    if s.bold {
        font.weight = FontWeight::BOLD;
    }
    if s.italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len,
        font,
        color: to_hsla(s.fg),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}
