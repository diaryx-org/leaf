//! The browser demo's binding: one Markdown document, the formula being
//! edited, and the keys and clicks that edit it.

use crate::doc;
use crate::edit::{Key, Session};
use std::ops::Range;
use twig::Format;
use wasm_bindgen::prelude::*;

/// Pixels per em the demo draws at.
const SIZE: f64 = 44.0;
/// [`crate::layout::Layout::svg`]'s padding.
const PAD: f64 = 4.0;

#[wasm_bindgen]
pub struct Demo {
    ed: twig::Editor,
    tex: Range<usize>,
    display: bool,
    session: Session,
}

#[wasm_bindgen]
impl Demo {
    /// Open `markdown` and start editing its `index`th formula, caret at
    /// its end.
    #[wasm_bindgen(constructor)]
    pub fn new(markdown: &str, index: usize) -> Result<Demo, JsError> {
        console_error_panic_hook::set_once();
        let mut ed =
            doc::editor(markdown, Format::Markdown).map_err(|e| JsError::new(&e.to_string()))?;
        let f = doc::formulas(&mut ed)
            .map_err(|e| JsError::new(&e.to_string()))?
            .into_iter()
            .nth(index)
            .ok_or_else(|| JsError::new("no such formula"))?;
        let source = ed.source_str().map_err(|e| JsError::new(&e.to_string()))?;
        let session = Session::new(&source[f.tex.clone()]).map_err(|e| JsError::new(&e.message))?;
        Ok(Demo {
            ed,
            tex: f.tex,
            display: f.display,
            session,
        })
    }

    /// How many formulas `markdown` holds.
    pub fn count(markdown: &str) -> usize {
        doc::editor(markdown, Format::Markdown)
            .ok()
            .and_then(|mut ed| doc::formulas(&mut ed).ok())
            .map_or(0, |f| f.len())
    }

    /// A key, by its DOM `KeyboardEvent.key`. Returns whether it was used.
    pub fn key(&mut self, key: &str) -> bool {
        let k = match key {
            "ArrowLeft" => Key::Left,
            "ArrowRight" => Key::Right,
            "ArrowUp" => Key::Up,
            "ArrowDown" => Key::Down,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            k if k.chars().count() == 1 => Key::Char(k.chars().next().unwrap()),
            _ => return false,
        };
        for s in self.session.key(k) {
            match doc::apply(&mut self.ed, &self.tex, &s) {
                Ok(t) => self.tex = t,
                Err(_) => return true,
            }
        }
        true
    }

    /// A click at `(x, y)` in the SVG's pixels.
    pub fn click(&mut self, x: f64, y: f64) {
        if let Ok(l) = self.session.formula.layout(self.display) {
            let (ex, ey) = ((x - PAD) / SIZE, (y - PAD) / SIZE);
            self.session.caret = l.hit(&self.session.formula, ex, ey);
        }
    }

    pub fn svg(&self) -> String {
        match self.session.formula.layout(self.display) {
            Ok(l) => l.svg(&self.session.formula, SIZE, Some(self.session.caret)),
            Err(e) => format!("<pre>{e}</pre>"),
        }
    }

    pub fn tex(&self) -> String {
        self.session.tex().to_string()
    }

    /// The document's source, the formula's TeX marked with `\u{1}` and
    /// `\u{2}` so the page can highlight it.
    pub fn document(&mut self) -> String {
        let s = self.ed.source_str().unwrap_or_default();
        format!(
            "{}\u{1}{}\u{2}{}",
            &s[..self.tex.start],
            &s[self.tex.clone()],
            &s[self.tex.end..]
        )
    }
}
