//! The WebAssembly door onto [`leaf_math`] and [`leaf_session`], in a module
//! of its own: [`typeset_math`], and [`EditSession`], the editors leaf runs
//! for a region, chosen by its kind.
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

/// An [`EditSession`]'s picture: [`MathPicture`]'s fields, in em, with no
/// caret or selection drawn in it, and where those stand on it — `caret` as
/// `[x, y, width, height]` (a zero-width rect at the stop), `selection` as
/// four numbers a rect, in em from the picture's top left. A wasm-bindgen
/// class, for [`MathPicture`]'s reason.
#[wasm_bindgen(getter_with_clone)]
pub struct EditPicture {
    pub svg: String,
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    pub caret: Vec<f64>,
    pub selection: Vec<f64>,
}

/// One splice an [`EditSession`] made to its region's content: replace
/// `[from, to)` — byte offsets into the content as the splice before this one
/// left it — with `text`. A host adds the content's offset in its document
/// and applies it.
#[wasm_bindgen(getter_with_clone)]
pub struct EditSplice {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

/// A region open in an editor leaf runs itself, chosen by the region's kind
/// (`leaf-session`): a formula (`math`) edited as structure, where `/` makes
/// a fraction, `^` a superscript and `sqrt` a root. Each key answers with the
/// splices it made to the content, so the host's document stays the source
/// of truth and its undo history sees ordinary edits. One class for every
/// kind, as leaf-ffi's `EditSession` is.
#[wasm_bindgen]
pub struct EditSession {
    editor: Box<dyn leaf_session::Editor>,
}

/// The kinds of region an [`EditSession`] can be opened for.
#[wasm_bindgen]
pub fn edit_session_kinds() -> Vec<String> {
    leaf_session::KINDS.iter().map(|k| k.to_string()).collect()
}

#[wasm_bindgen]
impl EditSession {
    /// Open `content`, a region of `kind`, the caret at its end. `display` is
    /// a formula's style. An error for a kind no editor here edits, or
    /// content its editor cannot read, which a host answers by leaving the
    /// region to be edited as source.
    #[wasm_bindgen(constructor)]
    pub fn new(kind: &str, content: &str, display: bool) -> Result<EditSession, JsValue> {
        let editor = leaf_session::open(kind, content, display)
            .map_err(|e| JsValue::from_str(&e.message))?;
        Ok(EditSession { editor })
    }

    /// A key, by its DOM `KeyboardEvent.key`: a printable character, an
    /// arrow, `Backspace` or `Delete` — or `ExtendLeft`, `ExtendRight` or
    /// `SelectAll`. Anything else is not the session's and makes no splice.
    pub fn key(&mut self, key: &str) -> Vec<EditSplice> {
        use leaf_session::Key;
        let k = match key {
            "ArrowLeft" => Key::Left,
            "ArrowRight" => Key::Right,
            "ArrowUp" => Key::Up,
            "ArrowDown" => Key::Down,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "ExtendLeft" => Key::ExtendLeft,
            "ExtendRight" => Key::ExtendRight,
            "SelectAll" => Key::SelectAll,
            k if k.chars().count() == 1 => Key::Char(k.chars().next().unwrap()),
            _ => return Vec::new(),
        };
        splices(self.editor.key(k))
    }

    /// Whether part of the region is selected.
    pub fn has_selection(&self) -> bool {
        self.editor.selected().is_some()
    }

    /// The selection as the content's own text — what a copy takes — or
    /// `undefined` with nothing selected.
    pub fn selected_text(&self) -> Option<String> {
        self.editor.selected()
    }

    /// Paste `text` at the caret, over the selection.
    pub fn paste(&mut self, text: &str) -> Vec<EditSplice> {
        splices(self.editor.paste(text))
    }

    /// The caret to the first stop, as → into the region from in front.
    pub fn home(&mut self) {
        self.editor.home();
    }

    /// The caret to the last stop, as ← into the region from behind.
    pub fn end(&mut self) {
        self.editor.end();
    }

    /// Whether ← would leave the region: the caret is at its first stop.
    pub fn at_home(&self) -> bool {
        self.editor.at_home()
    }

    /// Whether → would leave it.
    pub fn at_end(&self) -> bool {
        self.editor.at_end()
    }

    /// Take `content` as the region's after an undo or an outside change.
    pub fn reload(&mut self, content: &str) -> Result<(), JsValue> {
        self.editor
            .reload(content)
            .map_err(|e| JsValue::from_str(&e.message))
    }

    /// The caret to the stop nearest `(x, y)`, in pixels from the picture's
    /// top left at `size` px per em.
    pub fn click(&mut self, x: f64, y: f64, size: f64) {
        self.editor.click(x / size, y / size);
    }

    /// The content as the session holds it.
    pub fn content(&self) -> String {
        self.editor.content()
    }

    /// The region drawn in `color`, and where its caret and selection stand
    /// on it — [`typeset_math`]'s shape and more, so the host draws it where
    /// it would have drawn the renderer's picture, and its own caret and
    /// selection over it.
    pub fn picture(&self, size: f64, color: &str) -> Result<EditPicture, JsValue> {
        let ink = parse_hex_color(color)
            .ok_or_else(|| JsValue::from_str(&format!("not a hex colour: {color}")))?;
        let p = self
            .editor
            .picture(&leaf_session::Style { size, ink })
            .map_err(|e| JsValue::from_str(&e.message))?;
        let rect = |r: &leaf_session::Rect| [r.x, r.y, r.width, r.height];
        Ok(EditPicture {
            svg: p.svg,
            width: p.width,
            height: p.height,
            depth: p.depth,
            caret: rect(&p.caret).to_vec(),
            selection: p.selection.iter().flat_map(rect).collect(),
        })
    }
}

/// A session's splices as the binding hands them over.
fn splices(list: Vec<leaf_session::Splice>) -> Vec<EditSplice> {
    list.into_iter()
        .map(|s| EditSplice {
            from: s.range.start,
            to: s.range.end,
            text: s.text,
        })
        .collect()
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
    fn a_session_answers_keys_with_splices_on_its_content() {
        assert_eq!(edit_session_kinds(), vec!["math".to_string()]);
        let mut s = EditSession::new("math", "x", false).unwrap();
        let sp = s.key("/");
        assert_eq!(sp.len(), 1);
        assert_eq!(
            (sp[0].from, sp[0].to, sp[0].text.as_str()),
            (0, 1, "\\frac{x}{}")
        );
        assert_eq!(s.content(), "\\frac{x}{}");
        assert!(s.key("Escape").is_empty());
        s.end();
        assert!(s.at_end() && !s.at_home());
        let p = s.picture(16.0, "#000").unwrap();
        assert_eq!(p.caret.len(), 4, "the caret is a rect");
        assert!(p.caret[3] > 0.0);
        assert!(p.selection.is_empty());
        // A selection: copied as TeX, pasted over, given as a rect.
        assert!(s.key("SelectAll").is_empty());
        assert!(s.has_selection());
        assert_eq!(s.selected_text().as_deref(), Some("\\frac{x}{}"));
        let selected = s.picture(16.0, "#000").unwrap();
        assert_eq!(selected.selection.len(), 4, "one selection rect");
        assert_eq!(
            selected.svg, p.svg,
            "the selection is drawn into the picture"
        );
        let sp = s.paste("y");
        assert_eq!(sp.len(), 2, "the selection's removal, then the paste");
        assert_eq!(s.content(), "y");
        assert!(!s.has_selection());
        s.home();
        assert_eq!(s.key("Delete").len(), 1);
        assert_eq!(s.content(), "");
    }

    #[test]
    fn a_formula_typesets_to_a_picture_with_its_metrics() {
        let p = typeset_math("x^2", false, 16.0, "#000").unwrap();
        assert!(p.svg.starts_with("<svg"));
        assert!(p.width > 0.0 && p.height > 0.0);
    }
}
