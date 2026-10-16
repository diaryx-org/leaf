//! The smallest host: the editor alone in a window, on a document in memory,
//! with the system clipboard and every request ignored.
//!
//! ```sh
//! cargo run --manifest-path crates/leaf-dioxus/Cargo.toml --example minimal
//! ```

use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_native::winit::event::WindowEvent;
use leaf_core::{Doc, Format};
use leaf_dioxus::{Host, Look, pressed, use_editor};

struct Mine;
impl Host for Mine {}

fn main() {
    dioxus_native::launch(App);
}

#[allow(non_snake_case)]
fn App() -> Element {
    let ed = use_editor(
        || {
            Doc::from_source("# Hello\n\nType here.\n".into(), Format::Markdown)
                .expect("a document")
        },
        Look::default,
        || Rc::new(Mine) as Rc<dyn Host>,
    );
    let mut mods = Default::default();
    dioxus_native::use_window_event({
        let ed = ed.clone();
        move |event, _| match event {
            WindowEvent::ModifiersChanged(m) => mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some((key, code, m)) = pressed(event, mods) {
                    ed.key(key, code, m);
                }
            }
            event => ed.window_event(event),
        }
    });
    ed.render()
}
