//! Leaf in a browser tab: the leaf-dioxus editor on a document in memory,
//! drawn by Blitz into a canvas the page fills.
//!
//! A host as `crates/leaf-dioxus/examples/minimal.rs` is one, with what a
//! browser does not give for free brought here:
//!
//! - **Faces.** A browser lists no system fonts to Blitz or to parley, so the
//!   page brings its own (`fonts/`) and hands the same files to both — the
//!   editor measures with them through [`Type::faces`], and Blitz draws with
//!   them through the collection [`font_context`] makes, and SVG text — a
//!   diagram's labels — through usvg's database (`add_svg_fonts`).
//! - **A clipboard.** arboard has no browser. Copy and Cut go to a clipboard
//!   in the page, and the text to the system's as well; Paste reads the
//!   page's, since reading the system's needs the `paste` event's data,
//!   which winit's canvas does not let through.
//!
//! Keys reach the editor through winit's canvas, as on the Mac. The input
//! method does not: winit's web backend has none, so a composition — an
//! accent's dead key, a phone's keyboard, Japanese — does not reach the
//! editor yet (README.md).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use dioxus::prelude::*;
use dioxus_native::winit::event::WindowEvent;
use dioxus_native::{Config, FontContext};
use leaf_core::{Doc, Format};
use leaf_dioxus::{Clipboard, Faces, Host, Look, Type, pressed, use_editor};
use parley::fontique::{Collection, CollectionOptions, GenericFamily, SourceCache};
use wasm_bindgen::prelude::*;

/// The faces the page brings, as `index.html` fetched them: Libertinus
/// Serif for the body, in the book face's four styles, and DejaVu Sans Mono
/// for code. Their licences are beside them in `fonts/`. Fetched beside the
/// wasm rather than compiled into it, so they download in parallel with it
/// and stay cached when it changes. Kept for the page's lifetime, which is
/// what lets every [`Faces`] made of them share the bytes.
static FILES: OnceLock<Vec<&'static [u8]>> = OnceLock::new();
const BODY: &str = "Libertinus Serif";
const MONO: &str = "DejaVu Sans Mono";

/// What the page opens on.
const DEMO: &str = include_str!("../demo.md");

/// Start the editor, with the font files `index.html` fetched, in order.
#[wasm_bindgen]
pub fn start(fonts: Vec<js_sys::Uint8Array>) {
    console_error_panic_hook::set_once();
    let files = fonts
        .iter()
        .map(|f| &*Vec::leak(f.to_vec()))
        .collect::<Vec<&'static [u8]>>();
    blitz_dom::add_svg_fonts(files.iter().map(|f| f.to_vec()));
    let _ = FILES.set(files);
    dioxus_native::launch_cfg(
        App,
        Vec::new(),
        vec![Box::new(
            Config::new().with_font_ctx(font_context(&faces())),
        )],
    );
}

/// The page's faces, as the editor measures with them. The files are
/// `'static`, so each `Faces` made of them shares the bytes.
fn faces() -> Faces {
    FILES.get().into_iter().flatten().copied().collect()
}

/// Blitz's font collection: the page's faces and nothing else, with every
/// generic family the stylesheets name — the editor's chrome is `system-ui`,
/// a fallback is `serif` or `monospace` — answered by one of them.
fn font_context(faces: &Faces) -> FontContext {
    let mut cx = FontContext {
        source_cache: SourceCache::new_shared(),
        collection: Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        }),
    };
    for blob in faces.blobs() {
        cx.collection.register_fonts(blob.clone(), None);
    }
    let family = |cx: &mut FontContext, name| cx.collection.family_by_name(name).map(|f| f.id());
    let body = family(&mut cx, BODY);
    let mono = family(&mut cx, MONO);
    for (generic, face) in [
        (GenericFamily::Serif, body),
        (GenericFamily::SansSerif, body),
        (GenericFamily::SystemUi, body),
        (GenericFamily::UiSerif, body),
        (GenericFamily::UiSansSerif, body),
        (GenericFamily::Monospace, mono),
        (GenericFamily::UiMonospace, mono),
    ] {
        cx.collection
            .set_generic_families(generic, face.into_iter());
    }
    cx
}

/// The editor's look on the page: the Mac's, in the page's faces.
fn look() -> Look {
    Look {
        ty: Type {
            // Libertinus has no ⌘, ⌥ or ⇧, which the key chords in a
            // document spell; DejaVu has them all.
            body: format!("\"{BODY}\", \"{MONO}\", serif"),
            mono: format!("\"{MONO}\", monospace"),
            faces: faces(),
            ..Type::default()
        },
        ..Look::default()
    }
}

/// The page's [`Host`]: a clipboard of its own, and links opened in a new
/// tab. The rest are the defaults: Save, Save As and the settings have
/// nowhere to go yet.
#[derive(Default)]
struct Page {
    board: RefCell<Clipboard>,
}

impl Host for Page {
    fn write_clipboard(&self, text: &str, html: Option<&str>) -> Result<(), String> {
        *self.board.borrow_mut() = Clipboard {
            text: Some(text.into()),
            html: html.map(Into::into),
        };
        // The system's too, as text, for a paste outside the page. The
        // promise settles later, and a refusal leaves the page's copy.
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(text);
        }
        Ok(())
    }

    fn read_clipboard(&self) -> Clipboard {
        self.board.borrow().clone()
    }

    /// A web address, in a new tab. Anything else names a file beside the
    /// document, and a document in memory has none.
    fn open(&self, what: &str) {
        let web = ["https:", "http:", "mailto:"]
            .iter()
            .any(|scheme| what.starts_with(scheme));
        if let (true, Some(window)) = (web, web_sys::window()) {
            let _ = window.open_with_url_and_target(what, "_blank");
        }
    }
}

#[allow(non_snake_case)]
fn App() -> Element {
    let ed = use_editor(
        || Doc::from_source(DEMO.into(), Format::Markdown).expect("the demo document"),
        look,
        || Rc::new(Page::default()) as Rc<dyn Host>,
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
