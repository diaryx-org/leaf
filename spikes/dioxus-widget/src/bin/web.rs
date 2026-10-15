//! The same widget in a browser, to answer the two questions the headless
//! bench cannot: does it build for `wasm32-unknown-unknown` at all, and what
//! does the module weigh next to `leaf-wasm` plus `leaf-web`'s JavaScript?
//!
//! ```sh
//! cargo build --release --target wasm32-unknown-unknown --features web --bin web
//! ```
//!
//! README.md's "The web" section has the sizes. Serving it needs `dx` or a
//! `wasm-bindgen` pass, neither of which the measurement depends on.

use dioxus::prelude::*;
use dioxus_widget_spike::LeafEditor;

const SAMPLE: &str = include_str!("../../corpus/document.md");

fn main() {
    console_error_panic_hook::set_once();
    dioxus_web::launch::launch(App, Vec::new(), Vec::new());
}

#[allow(non_snake_case)]
fn App() -> Element {
    rsx! {
        LeafEditor { source: SAMPLE.to_string() }
    }
}
