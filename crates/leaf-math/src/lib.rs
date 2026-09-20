//! TeX math, typeset to a picture leaf's frontends already know how to draw.
//!
//! One function: [`typeset`] takes the text between a formula's delimiters and
//! hands back a standalone SVG document plus the three numbers a frontend
//! needs to place it — `width`, `height` above the baseline, and `depth`
//! below it, in em. A display formula needs only the first two to size its
//! box; an inline one needs all three, because it sits *in* a line and the
//! text baseline has to pass through it at `height` from the top.
//!
//! The SVG is self-contained: every glyph is a `<path>` with KaTeX's own
//! outlines embedded at build time, so the picture is byte-identical on every
//! frontend and no font has to be found at runtime. Colour is an input, so a
//! theme change is a re-render; the render is pure layout over embedded fonts
//! — no I/O, and fast enough to run on a layout thread. Callers cache by
//! `(tex, display, size, colour)`.
//!
//! Nothing here is leaf's: it is RaTeX with leaf's conventions on the door
//! (pixels rather than points, a colour as four bytes, an error that names a
//! byte). If it grows a consumer outside leaf it moves out, the way
//! resvg-swift did.

use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_parser::parse;
use ratex_svg::{SvgColorSyntax, SvgOptions, render_to_svg_with_color_syntax};
use ratex_types::color::Color;
use ratex_types::math_style::MathStyle;
use std::fmt;

/// A typeset formula: the picture and where its baseline is.
#[derive(Clone, Debug, PartialEq)]
pub struct MathPicture {
    /// A standalone SVG document. Its `viewBox`, `width`, and `height` are in
    /// pixels at the `size` the formula was typeset at — `width * size` by
    /// `(height + depth) * size` — so a frontend that draws it at its intrinsic
    /// size lands the glyphs at the font size it asked for.
    pub svg: String,
    /// The advance width of the formula, in em.
    pub width: f64,
    /// How far the formula rises above its baseline, in em. The baseline of
    /// the surrounding text should pass through the picture this far from its
    /// top.
    pub height: f64,
    /// How far the formula reaches below its baseline, in em.
    pub depth: f64,
}

impl MathPicture {
    /// The picture's pixel width at the `size` it was typeset at.
    pub fn px_width(&self, size: f64) -> f64 {
        self.width * size
    }

    /// The picture's pixel height at the `size` it was typeset at — the
    /// ascent plus the descent.
    pub fn px_height(&self, size: f64) -> f64 {
        (self.height + self.depth) * size
    }
}

/// TeX the typesetter could not read. `position` is a byte offset into the
/// formula's text where the parser gave up, when it can say — so a frontend
/// can show the revealed source with the fault marked, rather than a blank
/// box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathError {
    pub message: String,
    pub position: Option<usize>,
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some(at) => write!(f, "{} (at byte {at})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for MathError {}

/// Typeset `tex` — the text between the delimiters, `$…$` and `$$…$$` alike
/// stripped — as a picture.
///
/// `display` chooses TeX's display style (limits above and below a `\sum`,
/// a full-height fraction) over text style, the same switch `$$` makes over
/// `$`. `size` is the font size in pixels the formula is set at: an inline
/// formula takes the size of the text around it, a display one the body size.
/// `color` is the ink, as `[r, g, b, a]` bytes.
///
/// Whitespace either side of `tex` is insignificant to TeX and is trimmed, so
/// a display block whose source keeps the formula on lines of its own reads
/// the same as one written on the delimiters' line.
pub fn typeset(
    tex: &str,
    display: bool,
    size: f64,
    color: [u8; 4],
) -> Result<MathPicture, MathError> {
    let ast = parse(tex.trim()).map_err(|e| MathError {
        message: e.message,
        position: e.loc.map(|l| l.start),
    })?;
    let style = if display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let [r, g, b, a] = color.map(|c| f32::from(c) / 255.0);
    let opts = LayoutOptions::default()
        .with_style(style)
        .with_color(Color::new(r, g, b, a));
    let list = to_display_list(&layout(&ast, &opts));
    let svg = render_to_svg_with_color_syntax(
        &list,
        &SvgOptions {
            font_size: size,
            padding: 0.0,
            stroke_width: (size / 16.0).max(0.5),
            embed_glyphs: true,
            font_dir: String::new(),
        },
        // `rgb()` plus an opacity attribute, which every SVG consumer leaf
        // has (resvg, Core Graphics through resvg-swift, the browser) reads;
        // `rgba()` in a fill is CSS Color 4 and not SVG 1.1.
        SvgColorSyntax::Rgb,
    );
    Ok(MathPicture {
        svg: in_pixels(svg),
        width: list.width,
        height: list.height,
        depth: list.depth,
    })
}

/// RaTeX writes the root's `width` and `height` in `pt`, so that a 40-unit em
/// prints at 40pt. leaf's frontends size a picture in pixels — an `<img>`, a
/// `CGSize`, a gpui `Pixels` — and a `pt` unit would have every one of them
/// scale it by 4/3 on the way in. Strip the unit: the viewBox is already the
/// pixel box, and an unqualified length is a user unit, which is a pixel.
fn in_pixels(svg: String) -> String {
    let Some(head_end) = svg.find('>') else {
        return svg;
    };
    let (head, rest) = svg.split_at(head_end);
    let mut head = head.to_string();
    for attr in ["width=\"", "height=\""] {
        if let Some(i) = head.find(attr) {
            let v = i + attr.len();
            if let Some(q) = head[v..].find('"') {
                let value = head[v..v + q].trim_end_matches("pt").to_string();
                head.replace_range(v..v + q, &value);
            }
        }
    }
    head.push_str(rest);
    head
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: [u8; 4] = [0, 0, 0, 255];

    #[test]
    fn an_inline_formula_is_a_standalone_svg_with_metrics() {
        let p = typeset("E = mc^2", false, 16.0, BLACK).unwrap();
        assert!(
            p.svg
                .starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\"")
        );
        assert!(p.svg.ends_with("</svg>"));
        // Glyphs are outlines, not font references.
        assert!(p.svg.contains("<path"), "{}", p.svg);
        assert!(!p.svg.contains("<text"), "{}", p.svg);
        assert!(!p.svg.contains("font-family"), "{}", p.svg);
        // `E = mc^2` is a few em wide, sits on its baseline, and has no descender.
        assert!(p.width > 3.0 && p.width < 5.0, "{}", p.width);
        assert!(p.height > 0.6 && p.height < 1.0, "{}", p.height);
        assert_eq!(p.depth, 0.0);
    }

    #[test]
    fn a_display_formula_has_a_depth_and_a_display_style() {
        let d = typeset(r"\sum_{i=0}^n i", true, 16.0, BLACK).unwrap();
        let t = typeset(r"\sum_{i=0}^n i", false, 16.0, BLACK).unwrap();
        // Display style sets the limits above and below the sum: taller and
        // narrower than text style, which sets them as sub/superscripts.
        assert!(d.height + d.depth > t.height + t.depth, "{d:?} vs {t:?}");
        assert!(d.width < t.width, "{d:?} vs {t:?}");
        assert!(d.depth > 0.0);
    }

    #[test]
    fn the_root_is_sized_in_pixels_at_the_requested_size() {
        let p = typeset("x", false, 20.0, BLACK).unwrap();
        // No `pt` anywhere in the root element.
        let head = &p.svg[..p.svg.find('>').unwrap()];
        assert!(!head.contains("pt"), "{head}");
        // And the width attribute is the em width scaled by the size.
        let attr = |name: &str| -> f64 {
            head.split(&format!("{name}=\""))
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        assert!((attr("width") - p.px_width(20.0)).abs() < 1e-3);
        assert!((attr("height") - p.px_height(20.0)).abs() < 1e-3);
    }

    #[test]
    fn colour_is_the_ink() {
        let p = typeset("x", false, 16.0, [255, 0, 0, 255]).unwrap();
        assert!(p.svg.contains("fill=\"rgb(255,0,0)\""), "{}", p.svg);
        assert!(!p.svg.contains("rgba("), "{}", p.svg);
        let p = typeset("x", false, 16.0, [0, 0, 255, 128]).unwrap();
        assert!(
            p.svg.contains("fill=\"rgb(0,0,255)\" fill-opacity=\"0.50"),
            "{}",
            p.svg
        );
    }

    #[test]
    fn surrounding_whitespace_is_insignificant() {
        let a = typeset("\n  x + y \n", false, 16.0, BLACK).unwrap();
        let b = typeset("x + y", false, 16.0, BLACK).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn unreadable_tex_is_an_error_that_names_a_byte() {
        let e = typeset(r"\frac{a", false, 16.0, BLACK).unwrap_err();
        assert!(!e.message.is_empty());
        assert!(e.position.is_some(), "{e}");
        assert!(e.to_string().contains("at byte"), "{e}");
    }

    #[test]
    fn an_empty_formula_is_an_empty_picture_not_an_error() {
        let p = typeset("", false, 16.0, BLACK).unwrap();
        assert_eq!(p.width, 0.0);
        assert!(p.svg.starts_with("<svg"));
    }
}
