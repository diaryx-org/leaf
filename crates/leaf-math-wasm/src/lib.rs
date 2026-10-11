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
/// One splice a [`MathSession`] keystroke made to its TeX: replace
/// `[from, to)` — byte offsets into the TeX as it stood before this splice —
/// with `text`. A host adds the TeX's offset in its document and applies it.
#[wasm_bindgen(getter_with_clone)]
pub struct MathSplice {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

/// A formula being edited as structure: typing `/` makes a fraction, `^` a
/// superscript, `sqrt` a root; the arrows walk the slots. Each keystroke
/// answers with the splices it made to the TeX, so the host's document stays
/// the source of truth and its undo history sees ordinary edits.
#[wasm_bindgen]
pub struct MathSession {
    session: leaf_math_edit::edit::Session,
}

#[wasm_bindgen]
impl MathSession {
    /// Open `tex` for editing, the caret at its end. An error when the TeX is
    /// beyond what the structural editor reads, which a host answers by
    /// leaving the formula to be edited as source.
    #[wasm_bindgen(constructor)]
    pub fn new(tex: &str, display: bool) -> Result<MathSession, JsValue> {
        let mut session =
            leaf_math_edit::edit::Session::new(tex).map_err(|e| JsValue::from_str(&e.message))?;
        session.display = display;
        Ok(MathSession { session })
    }

    /// A key, by its DOM `KeyboardEvent.key`: a printable character, an
    /// arrow, or `Backspace`. Anything else is not the session's and makes
    /// no splice.
    pub fn key(&mut self, key: &str) -> Vec<MathSplice> {
        use leaf_math_edit::edit::Key;
        let k = match key {
            "ArrowLeft" => Key::Left,
            "ArrowRight" => Key::Right,
            "ArrowUp" => Key::Up,
            "ArrowDown" => Key::Down,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "ShiftLeft" => Key::ShiftLeft,
            "ShiftRight" => Key::ShiftRight,
            "SelectAll" => Key::SelectAll,
            k if k.chars().count() == 1 => Key::Char(k.chars().next().unwrap()),
            _ => return Vec::new(),
        };
        splices(self.session.key(k))
    }

    /// Whether part of the formula is selected.
    pub fn has_selection(&self) -> bool {
        self.session.selection().is_some()
    }

    /// The selected part's TeX, as written — what a copy takes — or
    /// `undefined` with nothing selected.
    pub fn selected_tex(&self) -> Option<String> {
        self.session.selected_tex().map(str::to_string)
    }

    /// Paste `tex` at the caret, over the selection: written as it is where
    /// the formula reads with it, typed a character at a time where not.
    pub fn paste(&mut self, tex: &str) -> Vec<MathSplice> {
        splices(self.session.paste(tex))
    }

    /// The caret to the first stop, as → into the formula from in front.
    pub fn home(&mut self) {
        self.session.home();
    }

    /// The caret to the last stop, as ← into the formula from behind.
    pub fn end(&mut self) {
        self.session.end();
    }

    /// Whether ← would leave the formula: the caret is at its first stop.
    pub fn at_home(&self) -> bool {
        self.session.at_home()
    }

    /// Whether → would leave it.
    pub fn at_end(&self) -> bool {
        self.session.at_end()
    }

    /// Take `tex` as the formula's source after an undo or an outside change.
    pub fn reload(&mut self, tex: &str) -> Result<(), JsValue> {
        self.session
            .reload(tex)
            .map_err(|e| JsValue::from_str(&e.message))
    }

    /// The caret to the stop nearest `(x, y)`, in pixels from the picture's
    /// top left at `size` px per em.
    pub fn click(&mut self, x: f64, y: f64, size: f64) {
        self.session.click(x / size, y / size);
    }

    pub fn tex(&self) -> String {
        self.session.formula.src.clone()
    }

    /// The formula with its caret, in `color` with empty slots faint and the
    /// caret in `accent` — the shape [`typeset_math`] returns, so the host
    /// draws it where it would have drawn the picture.
    pub fn picture(&self, size: f64, color: &str, accent: &str) -> Result<MathPicture, JsValue> {
        let ink = parse_hex_color(color)
            .ok_or_else(|| JsValue::from_str(&format!("not a hex colour: {color}")))?;
        let accent = parse_hex_color(accent).unwrap_or([37, 99, 235, 255]);
        let style = leaf_math_edit::layout::Style {
            size,
            pad: 0.0,
            ink,
            faint: [ink[0], ink[1], ink[2], ink[3] / 3],
            accent,
        };
        let p = self
            .session
            .picture(&style)
            .map_err(|e| JsValue::from_str(&e))?;
        Ok(MathPicture {
            svg: p.svg,
            width: p.width,
            height: p.height,
            depth: p.depth,
        })
    }
}

/// A session's splices as the binding hands them over.
fn splices(list: Vec<leaf_math_edit::edit::Splice>) -> Vec<MathSplice> {
    list.into_iter()
        .map(|s| MathSplice {
            from: s.range.start,
            to: s.range.end,
            text: s.text,
        })
        .collect()
}

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
    fn a_session_answers_keys_with_splices_on_its_tex() {
        let mut s = MathSession::new("x", false).unwrap();
        let sp = s.key("/");
        assert_eq!(sp.len(), 1);
        assert_eq!(
            (sp[0].from, sp[0].to, sp[0].text.as_str()),
            (0, 1, "\\frac{x}{}")
        );
        assert_eq!(s.tex(), "\\frac{x}{}");
        assert!(s.key("Escape").is_empty());
        s.end();
        assert!(s.at_end() && !s.at_home());
        let p = s.picture(16.0, "#000", "#00f").unwrap();
        assert!(p.svg.contains("<rect"), "the caret is drawn");
        // A selection: copied as TeX, pasted over, shaded in the picture.
        assert!(s.key("SelectAll").is_empty());
        assert!(s.has_selection());
        assert_eq!(s.selected_tex().as_deref(), Some("\\frac{x}{}"));
        assert!(
            s.picture(16.0, "#000", "#00f")
                .unwrap()
                .svg
                .contains("fill-opacity")
        );
        let sp = s.paste("y");
        assert_eq!(sp.len(), 2, "the selection's removal, then the paste");
        assert_eq!(s.tex(), "y");
        assert!(!s.has_selection());
        s.home();
        assert_eq!(s.key("Delete").len(), 1);
        assert_eq!(s.tex(), "");
    }

    #[test]
    fn a_formula_typesets_to_a_picture_with_its_metrics() {
        let p = typeset_math("x^2", false, 16.0, "#000").unwrap();
        assert!(p.svg.starts_with("<svg"));
        assert!(p.width > 0.0 && p.height > 0.0);
    }
}
