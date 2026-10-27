//! Linux and Windows: no document system, menu bar or spelling service the
//! app can lean on, so it keeps its files itself.
//!
//! - [`document`]: `Document`, a file read and written by the app, which
//!   asks before a window with unsaved changes closes.
//! - [`print`]: Export as PDF, through the system's save dialog.
//! - [`spelling`] and [`lookup`]: neither, yet; each answers as a system
//!   with no checker and no dictionary would.
//!
//! And here: the File menu's commands, which the Mac's menu bar gives the
//! document system, and their keys ([`shortcut`]), since there is no menu
//! bar to own them; the system's dialogs, through `rfd` (the XDG desktop
//! portal on Linux); the clipboard, through `arboard`; and the settings, in
//! a file under the user's config folder.
//!
//! Only Linux has been run. Windows compiles the same code, untried.

pub mod document;
pub mod lookup;
pub mod print;
pub mod spelling;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use dioxus::html::{Code, Modifiers};
use dioxus_native::winit::window::{WindowAttributes, WindowId};
use leaf_core::Format;

use crate::app::{self, AppEvent};
use crate::page::Page;

/// One document's handle, as the shared code holds it.
pub type DocumentRef = Rc<document::Document>;

/// Run `f` on the handler's next turn.
pub fn later(f: impl FnOnce() + 'static) {
    app::on_turn(f);
}

/// The event loop's own settings: winit's defaults.
pub fn event_loop(_builder: &mut dioxus_native::winit::event_loop::EventLoopBuilder) {}

/// A document window's attributes: the window manager's own title bar.
pub fn document_window(attributes: WindowAttributes) -> WindowAttributes {
    attributes
}

/// How far a document window's page runs up behind the title bar: not at
/// all, since the title bar is the window manager's.
pub const TITLE_BAR: f64 = 0.0;

/// The page in the window that last had focus.
pub fn main_page() -> Option<WindowId> {
    app::last_focused().filter(|id| app::is_open(*id))
}

/// Before the loop runs: nothing to set up.
pub fn start() {}

/// The loop is running: open what the command line names, or an untitled
/// document — an app with no window here has nothing to show it is running.
pub fn launched() {
    let mut opened = false;
    if std::env::args().any(|a| a == "--sample") {
        open_sample();
        opened = true;
    }
    for file in std::env::args().skip(1).filter(|a| !a.starts_with('-')) {
        opened |= open_file(Path::new(&file));
    }
    if !opened {
        new_document(Format::Markdown);
    }
}

/// Open `path`: a window of its own, or the one it already has brought
/// forward. Whether a window opened or came forward.
pub fn open_file(path: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let open = app::pages().into_iter().find(|p| {
        p.shell()
            .is_some_and(|s| s.document.file().as_deref() == Some(path.as_path()))
    });
    if let Some(p) = open {
        p.window.focus_window();
        return true;
    }
    match document::Document::read(&path) {
        Ok(doc) => {
            app::post(AppEvent::Show(doc));
            true
        }
        Err(e) => {
            eprintln!("{e}");
            if let Some(p) = main_page().and_then(app::page) {
                p.say(&format!("Open failed: {e}"));
            }
            false
        }
    }
}

/// File ▸ Open…: the system's open dialog, then each file chosen.
pub fn open_panel() {
    let folder = main_page()
        .and_then(app::page)
        .and_then(|p| p.shell().and_then(|s| s.document.file()))
        .and_then(|f| f.parent().map(Path::to_path_buf));
    spawn_dialog(
        move || {
            let mut d = rfd::AsyncFileDialog::new().set_title("Open");
            let all: Vec<&str> = document::TYPES
                .iter()
                .flat_map(|(_, _, e)| e.iter().copied())
                .collect();
            d = d.add_filter("Documents", &all);
            for (_, name, exts) in document::TYPES {
                d = d.add_filter(name, exts);
            }
            if let Some(f) = folder {
                d = d.set_directory(f);
            }
            async move {
                d.pick_files()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|h| h.path().to_path_buf())
                    .collect::<Vec<_>>()
            }
        },
        |paths| {
            for p in paths {
                open_file(&p);
            }
        },
    );
}

/// The system's save dialog, suggesting `name` in `folder` for a file of
/// `kind` (its name and extensions); `then` the path chosen, on the
/// handler's turn. Nothing if the dialog is cancelled.
pub fn save_dialog(
    name: &str,
    folder: Option<PathBuf>,
    kind: (&'static str, &'static [&'static str]),
    then: impl FnOnce(PathBuf) + 'static,
) {
    let name = name.to_string();
    spawn_dialog(
        move || {
            let mut d = rfd::AsyncFileDialog::new()
                .set_title("Save")
                .set_file_name(name)
                .add_filter(kind.0, kind.1);
            if let Some(f) = folder {
                d = d.set_directory(f);
            }
            async move { d.save_file().await.map(|h| h.path().to_path_buf()) }
        },
        move |path| {
            if let Some(p) = path {
                then(p);
            }
        },
    );
}

/// Run a dialog off the handler, so the windows keep drawing while it is
/// up, and hand its answer back on the handler's turn.
fn spawn_dialog<T, F>(make: impl FnOnce() -> F, then: impl FnOnce(T) + 'static)
where
    T: Send + 'static,
    F: std::future::Future<Output = T> + Send + 'static,
{
    let future = make();
    let token = app::park(then);
    tokio::spawn(async move {
        let answer = future.await;
        app::unpark(token, answer);
    });
}

/// File ▸ New: an untitled document of `format`.
pub fn new_document(format: Format) {
    app::post(AppEvent::Show(document::Document::untitled(format, false)));
}

/// Help ▸ Open Sample Document: an untitled Markdown document holding it.
pub fn open_sample() {
    app::post(AppEvent::Show(document::Document::untitled(
        Format::Markdown,
        true,
    )));
}

/// File ▸ Quit: each document window asked to close, as its close button
/// would; the app quits when the last has gone.
pub fn quit() {
    for p in app::pages() {
        p.close_requested();
    }
}

/// Whether the app quits when its last document window closes: it does,
/// since without a menu bar nothing would be left to reach it by.
pub const QUITS_WITH_LAST_WINDOW: bool = true;

/// The keys of the commands the Mac's menu bar would answer, and the
/// editor does not: whether `code` with `mods` was one, done.
pub fn shortcut(page: &Page, code: &Code, mods: Modifiers) -> bool {
    let Some(c) = crate::menus::file()
        .into_iter()
        .chain(crate::menus::keyed())
        .filter_map(|n| match n {
            crate::menus::Node::Item(_, c) => Some(c),
            _ => None,
        })
        .find(|c| c.shortcut() == Some((*code, mods)))
    else {
        return false;
    };
    let id = page.window.id();
    app::post(AppEvent::Command(id, c));
    true
}

/// Where the app keeps its resources — the sample and its pictures, side
/// by side: `resources` beside the binary when installed, the app's own
/// when run from `cargo run`.
pub fn resources() -> PathBuf {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("resources")));
    match installed {
        Some(dir) if dir.join("Sample.md").exists() => dir,
        _ => PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/Media")),
    }
}

/// The bundled sample document's text.
pub fn sample() -> String {
    let dev = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/Sample.md");
    std::fs::read_to_string(resources().join("Sample.md"))
        .or_else(|_| std::fs::read_to_string(dev))
        .unwrap_or_else(|_| "# leaf\n\nThe sample document is missing.\n".into())
}

/// Hand a URL, or a file, to the app the system opens it with.
pub fn open_url(what: &str) {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = std::process::Command::new("xdg-open");
    if let Err(e) = cmd.arg(what).spawn() {
        eprintln!("open {what}: {e}");
    }
}

/// The emoji and symbols picker: the system's own shortcut opens it, where
/// there is one.
pub fn character_palette() {}

/// Every font family installed.
pub fn font_families() -> Vec<String> {
    let mut collection = parley::fontique::Collection::new(parley::fontique::CollectionOptions {
        shared: false,
        system_fonts: true,
    });
    let mut names: Vec<String> = collection
        .family_names()
        .filter(|n| !n.starts_with('.'))
        .map(String::from)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// The app's settings, in a file: `<config>/leaf/settings`, a line per key,
/// the key and its value separated by a tab.
pub mod defaults {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn file() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("leaf").join("settings"))
    }

    fn load() -> BTreeMap<String, String> {
        let text = file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        parse(&text)
    }

    fn parse(text: &str) -> BTreeMap<String, String> {
        text.lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn store(map: &BTreeMap<String, String>) {
        let Some(f) = file() else { return };
        let text: String = map
            .iter()
            .map(|(k, v)| format!("{k}\t{}\n", v.replace(['\t', '\n'], " ")))
            .collect();
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&f, text) {
            eprintln!("{}: {e}", f.display());
        }
    }

    /// The value under `key`, as a string: a Bool reads as "0" or "1".
    pub fn string(key: &str) -> Option<String> {
        load().remove(key)
    }

    pub fn set(key: &str, value: &str) {
        let mut map = load();
        map.insert(key.into(), value.into());
        store(&map);
    }

    pub fn set_bool(key: &str, value: bool) {
        set(key, if value { "1" } else { "0" });
    }

    pub fn remove(key: &str) {
        let mut map = load();
        if map.remove(key).is_some() {
            store(&map);
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn a_line_is_a_key_and_its_value() {
            let map =
                super::parse("display.paper\ta4\nbroken line\ndisplay.body\tIowan Old Style\n");
            assert_eq!(map.get("display.paper").map(String::as_str), Some("a4"));
            assert_eq!(
                map.get("display.body").map(String::as_str),
                Some("Iowan Old Style")
            );
            assert_eq!(map.len(), 2);
        }
    }
}

/// The system clipboard, with the HTML flavour beside the text, as the
/// editor publishes both.
///
/// One clipboard for the life of the app: on X11 the copied text is served
/// by the app that owns the selection, and dropping the last
/// `arboard::Clipboard` gives that up — with no clipboard manager to take
/// it, what was copied (or cut, and so gone from the document) would be
/// gone. [`clipboard::release`] hands it over at quit.
pub mod clipboard {
    use std::cell::RefCell;

    thread_local! {
        static BOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
    }

    /// `f` with the app's clipboard, made the first time it is asked for.
    fn with<R>(f: impl FnOnce(&mut arboard::Clipboard) -> R) -> Result<R, String> {
        BOARD.with_borrow_mut(|b| {
            if b.is_none() {
                *b = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
            }
            Ok(f(b.as_mut().expect("made above")))
        })
    }

    pub fn write(text: &str, html: Option<&str>) -> Result<(), String> {
        with(|board| match html {
            Some(html) => board.set_html(html, Some(text)),
            None => board.set_text(text),
        })?
        .map_err(|e| e.to_string())
    }

    pub fn read() -> leaf_dioxus::Clipboard {
        with(|board| leaf_dioxus::Clipboard {
            text: board.get_text().ok(),
            html: board.get().html().ok(),
        })
        .unwrap_or_default()
    }

    /// The app is quitting: give what was copied to a clipboard manager,
    /// where there is one, before the selection's owner goes.
    pub fn release() {
        BOARD.with_borrow_mut(|b| drop(b.take()));
    }
}
