use wasm_bindgen::prelude::*;

/// A Mermaid diagram as SVG, or the error as a string.
#[wasm_bindgen]
pub fn render_mermaid(source: &str) -> Result<String, String> {
    console_error_panic_hook::set_once();
    mermaid_rs_renderer::render(source).map_err(|e| format!("{e:#}"))
}
