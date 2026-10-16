//! Leaf on Dioxus's **native backend**
//! (`dioxus-native` 0.8.0-alpha.2, Blitz): the editor widget,
//! [`leaf_dioxus`], in a Mac document app.
//!
//! ```sh
//! cargo run --release -- corpus/document.md   # in apps/leaf-dioxus
//! ```
//!
//! The editor — its rows, keys, input method, find bar, source view and
//! status line — is the widget's (`crates/leaf-dioxus`). This is what a
//! document app adds around it, and what it answers the widget's `Host` with:
//!
//! - [`shell`]: the `Host` — Save, the context menu, the window title — and
//!   what the menu bar's items do.
//! - [`menu`]: the menu bar, every editor item a key chord replayed.
//! - [`file`]: New, Open, Save As, and the unsaved-changes question.
//! - [`settings`]: the display choices and the zoom; [`prefs`] puts them in
//!   effect and draws the panel (⌘,).
//!
//! `LEAF_TRACE=1` logs keys, pointer events and repaint timings to stderr;
//! `LEAF_OUTLINE=1` outlines every visual line and run.

mod file;
mod menu;
mod prefs;
mod settings;
mod shell;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_native::winit;
use dioxus_native::winit::event::WindowEvent;
use leaf_core::{Doc, Format};
use leaf_dioxus::{Host, pressed, themed, use_editor};

use crate::prefs::PrefsView;
use crate::settings::Settings;
use crate::shell::Shell;

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
    let source = include_str!("../corpus/document.md").to_string();
    Doc::from_source(source, Format::Markdown)
        .unwrap_or_else(|_| Doc::blank().expect("a blank document"))
}

/// The app: the editor, the shell around it, and every event source wired
/// to the two.
#[allow(non_snake_case)]
fn App() -> Element {
    let path = use_context::<Opened>().0;
    let window = dioxus_native::use_window();
    let prefs = use_signal(PrefsView::default);
    let shell = use_hook(|| Rc::new(Shell::new(window, Settings::load(), prefs)));
    let ed = use_editor(
        || open(path),
        || shell.look(),
        || shell.clone() as Rc<dyn Host>,
    );

    // The window's appearance, and what the settings panel offers, once.
    use_hook(|| shell.apply_settings(&ed));

    // The menu bar, installed once; a task takes what its items ask for. muda
    // queues each choice, and a shortcut that names an item is the item's —
    // macOS gives it to the menu before the view — so this is also where most
    // ⌘ chords arrive.
    use_hook({
        let (ed, shell) = (ed.clone(), shell.clone());
        move || {
            let menu = menu::install();
            spawn(async move {
                // What the Table menu's state was last set from.
                let mut seen = None;
                loop {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    while let Some(command) = menu.next() {
                        shell.command(&ed, command);
                    }
                    // The Table menu's grid items follow the caret in and out
                    // of a table, asked only when the caret or the text moved.
                    let mut doc = ed.doc();
                    let now = (doc.caret, doc.revision());
                    if seen != Some(now) {
                        seen = Some(now);
                        menu.set_in_table(doc.caret_in_table());
                    }
                }
            });
        }
    });

    // The window's own events: the keys, which the shell sees first (see
    // the editor's `keys.rs` for why not Blitz's), and the rest the
    // editor's — the input method, its size, whether it is covered.
    let mut mods = winit::keyboard::ModifiersState::empty();
    dioxus_native::use_window_event({
        let (ed, shell) = (ed.clone(), shell.clone());
        move |event, _| match event {
            WindowEvent::ModifiersChanged(m) => mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some((key, code, m)) = pressed(event, mods) {
                    shell.key(&ed, key, code, m);
                }
            }
            event => ed.window_event(event),
        }
    });

    let v = prefs.read().clone();
    rsx! {
        {ed.render()}
        style { {themed(prefs::CSS, v.settings.appearance)} }
        {prefs::render(&ed, &shell, &v)}
    }
}
