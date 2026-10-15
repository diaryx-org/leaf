//! The same widget in a real webview — which is what Dioxus is on desktop and
//! mobile: `dioxus-desktop` is wry, so WKWebView on Apple, WebKitGTK on Linux,
//! Android WebView on Android. This is the binary screenshotted in README.md's
//! "Webview against native" section, against `apps/leaf` on gpui.
//!
//! ```sh
//! cargo run --features desktop --bin desktop -- corpus/document.md
//! ```
//!
//! The window is a fixed logical size so the two screenshots are the same
//! shape, and the document is the one both renderers are given.

use dioxus::prelude::*;
use dioxus_desktop::{Config, LogicalSize, WindowBuilder};
use dioxus_widget_spike::LeafEditor;

/// The window both this and the gpui app are screenshotted at.
const SIZE: (f64, f64) = (1000.0, 700.0);

fn main() {
    let path = std::env::args().nth(1);
    let source = path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| include_str!("../../corpus/document.md").to_string());

    let window = WindowBuilder::new()
        .with_title("leaf - Dioxus (webview)")
        .with_inner_size(LogicalSize::new(SIZE.0, SIZE.1));

    dioxus_desktop::launch::launch(
        App,
        vec![Box::new(move || Box::new(Source(source.clone())))],
        vec![Box::new(Config::new().with_window(window).with_menu(None))],
    );
}

/// The document, handed in as a root context so `App` can stay a `fn()`.
#[derive(Clone)]
struct Source(String);

#[allow(non_snake_case)]
fn App() -> Element {
    let source = use_context::<Source>().0;
    rsx! {
        LeafEditor { source }
    }
}
