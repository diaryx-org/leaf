//! The WebAssembly door onto [`leaf_math`]: one function, [`typeset_math`],
//! in a module of its own.
//!
//! It is a separate module from `leaf-wasm` for one reason — weight. The
//! typesetter and its embedded fonts are about two megabytes of a binding
//! that was under four without them, and most documents never show a
//! formula. So the editor's own module knows nothing of TeX; it publishes
//! each formula as a `MathView` (its TeX, its style, where its picture goes)
//! and the editor fetches *this* module the first time a frame carries one,
//! typesets through it, and repaints. A page with no formulas never loads
//! it. `packages/leaf-web/src/editor.js` is where that happens.
//!
//! The contract is the same on both sides of the split: pure layout over
//! embedded fonts, no I/O, a picture that is byte-identical to the one every
//! other leaf frontend draws for the same TeX.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

/// A typeset formula: a standalone SVG document and where its baseline is.
/// See [`typeset_math`].
///
/// A wasm-bindgen class rather than a serde struct, deliberately: four
/// fields do not need a serializer, and serde plus its JS bridge would be a
/// hundred kilobytes of runtime this module carried twice over with
/// `leaf-wasm`'s. The JS side reads the four getters once and calls `free()`,
/// so the SVG does not stay in this module's heap.
#[wasm_bindgen(getter_with_clone)]
pub struct MathPicture {
    /// A self-contained SVG — every glyph an outline, no font to load. Its
    /// `viewBox`, `width` and `height` are in CSS pixels at the size it was
    /// typeset at, so dropped into the page at its intrinsic size the glyphs
    /// land at that font size.
    pub svg: String,
    /// The picture's advance width, in em of the size it was typeset at.
    pub width: f64,
    /// How far it rises above its baseline, in em.
    pub height: f64,
    /// How far it reaches below its baseline, in em — what an inline element
    /// is shifted down by (`vertical-align: -{depth}em`) so the baselines
    /// agree.
    pub depth: f64,
}

/// Typeset `tex` — the text between a formula's delimiters, as `leaf-wasm`'s
/// `MathView` hands it over — to a picture. `display` is the view's
/// `display`; `size` is the font size in CSS pixels the formula is set at (an
/// inline formula takes its run's, a block the body's); `color` is the ink as
/// CSS hex, `#rgb`, `#rrggbb` or `#rrggbbaa`. A theme change is a re-render
/// with a new colour.
///
/// Pure layout over fonts embedded in the module — no fetch, no font to
/// load, fast enough to call while painting a frame — so the renderer caches
/// by `(tex, display, size, colour)` and nothing more. TeX the typesetter
/// cannot read rejects with its message; the renderer shows the revealed
/// source in its place.
#[wasm_bindgen]
pub fn typeset_math(
    tex: &str,
    display: bool,
    size: f64,
    color: &str,
) -> Result<MathPicture, JsValue> {
    let rgba = parse_hex_color(color)
        .ok_or_else(|| JsValue::from_str(&format!("not a hex colour: {color}")))?;
    let p = leaf_math::typeset(tex, display, size, rgba)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(MathPicture {
        svg: p.svg,
        width: p.width,
        height: p.height,
        depth: p.depth,
    })
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` to bytes — the one colour syntax
/// [`typeset_math`] reads, because it is the one a stylesheet's custom
/// property most often resolves to and the one JS can produce without a
/// canvas.
fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let hex = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        3 => {
            let n = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
            Some([n(0)?, n(1)?, n(2)?, 255])
        }
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colours_read_three_six_and_eight_digits() {
        assert_eq!(parse_hex_color("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_hex_color("#1a2B3c"), Some([0x1a, 0x2b, 0x3c, 255]));
        assert_eq!(parse_hex_color(" #00000080 "), Some([0, 0, 0, 0x80]));
        assert_eq!(parse_hex_color("red"), None);
        assert_eq!(parse_hex_color("#12345"), None);
    }

    #[test]
    fn a_formula_typesets_to_a_picture_with_its_metrics() {
        let p = typeset_math("x^2", false, 16.0, "#000").unwrap();
        assert!(p.svg.starts_with("<svg"));
        assert!(p.width > 0.0 && p.height > 0.0);
    }
}
