//! The same leaf rows through Dioxus's **experimental native backend**
//! (`dioxus-native` 0.8.0-alpha.2, Blitz): an HTML/CSS renderer on wgpu, with
//! no webview and no browser engine.
//!
//! ```sh
//! cargo run --release -- ../corpus/document.md
//! ```
//!
//! An editor, not just a first paint, working toward the Swift editor
//! (`apps/leaf-editor`). Where to look:
//!
//! - [`editor`]: the state, and the repaint that turns the document into
//!   rows — over the parent's `Frame`, so a keystroke renders one row and
//!   memoizes the rest — and only the rows in view ([`viewport`]).
//! - [`dispatch`]: where keys, the input method and menu commands arrive;
//!   [`input`] maps keys to commands, [`menu`] puts them in the menu bar.
//! - [`motion`]: caret geometry, visual-line motion, the pointer.
//! - [`text`]: proportional rows, wrapped and hit-tested by parley; [`row`]
//!   draws them; [`source`] builds the source view's.
//! - [`blocks`]: what is drawn in place of rows — tables ([`table`]) now,
//!   and media ([`media`]). **Adding a kind of block is that file.**
//! - [`link`]: ⌘-click on a link or footnote, and the context menu.
//! - [`find`] / [`findbar`], [`file`], [`host`], [`ime`], [`status`].
//! - [`settings`]: the display choices and the zoom; [`prefs`] puts them
//!   in effect and draws the panel (⌘,).
//! - [`view`]: the stylesheet — light and dark — and the page.
//!
//! `LEAF_TRACE=1` logs keys, pointer events and repaint timings to stderr;
//! `LEAF_OUTLINE=1` outlines every visual line and run.
//!
//! The markup is duplicated from the parent's `editor.rs` rather than shared,
//! because `rsx!` builds types from *its* Dioxus and the two versions cannot
//! meet. Everything underneath it — the glyph→run coalescing, the class
//! mapping, the frame delta — is the parent crate's, which is the point: the
//! bridge is independent of which Dioxus renders it.

mod blocks;
mod dispatch;
mod editor;
mod file;
mod find;
mod findbar;
mod host;
mod ime;
mod input;
mod keys;
mod media;
mod link;
mod menu;
mod motion;
mod prefs;
mod row;
mod settings;
mod source;
mod status;
mod table;
mod text;
mod trace;
mod view;
mod viewport;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_native::winit;
use dioxus_native::winit::event::WindowEvent;
use leaf_core::{Doc, Format};

use crate::editor::{Editor, Signals};
use crate::row::PointerSink;
use crate::trace::trace;

fn main() {
    let path = std::env::args().nth(1).map(PathBuf::from);
    dioxus_native::launch_cfg(
        App,
        vec![Box::new(move || Box::new(Opened(path.clone())))],
        vec![],
    );
}

/// The file named on the command line, if any.
#[derive(Clone)]
struct Opened(Option<PathBuf>);

/// The document opened, or the corpus as an untitled one when no file was
/// named or it would not open.
fn open(path: Option<PathBuf>) -> Doc {
    if let Some(p) = path {
        match Doc::open(p.clone()) {
            Ok(d) => return d,
            Err(e) => eprintln!("{}: {e}", p.display()),
        }
    }
    let source = include_str!("../../corpus/document.md").to_string();
    Doc::from_source(source, Format::Markdown).unwrap_or_else(|_| Doc::blank().expect("a blank document"))
}

/// The app: the editor, and every event source wired to it.
#[allow(non_snake_case)]
fn App() -> Element {
    let path = use_context::<Opened>().0;
    let window = dioxus_native::use_window();
    let sig = Signals::new(&window);
    let ed = use_hook(|| Editor::new(open(path), window.clone(), sig));

    // The first paint, and the input method, allowed on the window once.
    use_hook({
        let ed = ed.clone();
        move || {
            ed.apply_settings();
            ed.enable_ime();
        }
    });

    // The pointer, from every line of every row and block.
    use_context_provider({
        let ed = ed.clone();
        move || PointerSink(Rc::new(move |p| ed.pointer(p)))
    });

    // The menu bar, installed once; a task takes what its items ask for. muda
    // queues each choice, and a shortcut that names an item is the item's —
    // macOS gives it to the menu before the view — so this is also where most
    // ⌘ chords arrive.
    use_hook({
        let ed = ed.clone();
        move || {
            let menu = menu::install();
            spawn(async move {
                // What the Table menu's state was last set from.
                let mut seen = None;
                loop {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    while let Some(command) = menu.next() {
                        ed.command(command);
                    }
                    // The Table menu's grid items follow the caret in and out
                    // of a table, asked only when the caret or the text moved.
                    let mut doc = ed.st.doc.borrow_mut();
                    let now = (doc.caret, doc.revision());
                    if seen != Some(now) {
                        seen = Some(now);
                        menu.set_in_table(doc.caret_in_table());
                    }
                }
            });
        }
    });

    // The window's own events: keys and the input method (see `keys.rs` for
    // why not Blitz's), its size, and whether it is covered.
    let mut mods = winit::keyboard::ModifiersState::empty();
    dioxus_native::use_window_event({
        let ed = ed.clone();
        move |event, _| match event {
            WindowEvent::Ime(ime) => ed.ime(ime),
            WindowEvent::SurfaceResized(size) => ed.resize(size.width, size.height),
            WindowEvent::ModifiersChanged(m) => mods = m.state(),
            // A click moved the caret: on the button's release, which winit
            // reports after Blitz has handled the press, tell the input method
            // where it went and what is around it now.
            WindowEvent::PointerButton { state, .. } if !state.is_pressed() => ed.ime_area(),
            // Blitz paints nothing while the window is covered, so a capture
            // of a covered window is its last frame — which looks exactly like
            // a hung editor. Said here so the trace tells them apart.
            WindowEvent::Occluded(o) => trace(|| format!("occluded {o}")),
            // Light or dark. Blitz turns the theme into the stylesheet's
            // `prefers-color-scheme` and restyles, but asks for no frame, so
            // the window would wait for the next event to show it.
            WindowEvent::ThemeChanged(theme) => {
                trace(|| format!("theme {theme:?}"));
                ed.st.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some((key, code, m)) = keys::pressed(event, mods) {
                    ed.key(key, code, m);
                    ed.ime_area();
                }
            }
            _ => {}
        }
    });

    view::render(&ed)
}
