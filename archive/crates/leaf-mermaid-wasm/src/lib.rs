//! `leaf-mermaid` for the browser: Mermaid diagrams to SVG, in a module a
//! host's fence editor loads on its own. The web editor package does not
//! import it; the demo page's Mermaid editor does.

use std::sync::Arc;

use wasm_bindgen::prelude::*;

/// A rendered diagram: a standalone SVG and its size in CSS pixels.
#[wasm_bindgen(getter_with_clone)]
pub struct MermaidPicture {
    pub svg: String,
    pub width: f64,
    pub height: f64,
}

/// The host's measuring function, `(text, size, family) => width`.
struct Measure(js_sys::Function);

// A wasm module runs on the one thread that loaded it, which is the only one
// that will ever call this.
unsafe impl Send for Measure {}
unsafe impl Sync for Measure {}

impl leaf_mermaid::Measure for Measure {
    fn width(&self, text: &str, size: f32, family: &str) -> Option<f32> {
        let w = self
            .0
            .call3(&JsValue::NULL, &text.into(), &size.into(), &family.into())
            .ok()?
            .as_f64()?;
        (w.is_finite() && w >= 0.0).then_some(w as f32)
    }
}

/// Render `source` — a ```` ```mermaid ```` fence's content — to SVG, in
/// Mermaid's light theme or its dark one. `measure(text, size, family)`
/// answers a label's width in pixels, as a canvas's `measureText` does for the
/// first face of `family` it has; without it the renderer estimates. A
/// diagram that does not parse throws the parser's message.
#[wasm_bindgen]
pub fn render_mermaid(
    source: &str,
    dark: bool,
    measure: Option<js_sys::Function>,
) -> Result<MermaidPicture, JsValue> {
    console_error_panic_hook::set_once();
    let measure = measure.map(|f| Arc::new(Measure(f)) as Arc<dyn leaf_mermaid::Measure>);
    let p =
        leaf_mermaid::render(source, dark, measure).map_err(|e| JsValue::from_str(&e.message))?;
    Ok(MermaidPicture {
        svg: p.svg,
        width: p.width,
        height: p.height,
    })
}
