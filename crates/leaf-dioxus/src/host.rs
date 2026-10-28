//! What the editor asks of whoever embeds it: the [`Host`].
//!
//! The editor owns the document, its rows and every key binding. What it does
//! not own is the world around it — the clipboard, the system's handler for a
//! URL, a native context menu, the window's title, and what Save means — so
//! it asks. Every method has a default, so `struct Mine; impl Host for Mine {}`
//! is a host: the system clipboard (with `system-clipboard`, the default),
//! pictures read by the editor's own rule, links and context menus that go
//! nowhere, and Save, Save As, the zoom and the settings ignored.
//! `apps/leaf-dioxus` answers all of them but where pictures are.
//!
//! The clipboard gestures themselves — what Copy, Cut and Paste do to the
//! document — are here, over the host's raw clipboard. Both flavors travel, as
//! leaf-swift publishes them: the source slice as `text/plain`, and
//! [`Doc::selection_html`] as `text/html`, so a paste into Mail or Docs keeps
//! its formatting. A paste prefers the HTML and falls back to the text, which
//! is [`Doc::paste_html`]'s own contract. A webview would need the `paste`
//! event's `DataTransfer` for these; native has no such restriction.

use std::cell::Cell;
use std::path::PathBuf;

use leaf_core::Doc;

use dioxus::prelude::*;

use crate::blocks::Directive;
use crate::editor::Editor;
use crate::input::{Gesture, ZoomCommand};
use crate::link::Context;

/// The lines of a document a host draws laid out its own way (Leaf's page
/// view: the document as its PDF sets it), for the moves that go by line:
/// up and down, by a screen, and to a line's edge. While a host gives the
/// editor some ([`Editor::set_lines`]), those moves are theirs, not the
/// rows'; every other key is still the editor's, on the same document.
pub trait Lines {
    /// The caret a line down or up, aiming for `goal`, the x a run of these
    /// keeps (set it on the first, read it after); the selection extended
    /// to it if `extend`.
    fn vertical(&self, doc: &mut Doc, down: bool, extend: bool, goal: &Cell<Option<f32>>);
    /// The caret a screen down or up, likewise.
    fn page(&self, doc: &mut Doc, down: bool, extend: bool, goal: &Cell<Option<f32>>);
    /// The caret to the start or the end of its line.
    fn line_edge(&self, doc: &mut Doc, end: bool, extend: bool);
}

/// What the clipboard holds, in the two flavors the editor reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clipboard {
    pub text: Option<String>,
    pub html: Option<String>,
}

/// A command the editor recognises — its key is one of the editor's
/// bindings — but leaves to the host to carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// ⌘S. A host whose documents are files saves to the document's path, or
    /// asks for one when it is untitled ([`Doc::is_untitled`]); one whose
    /// documents live elsewhere stores the source there.
    Save,
    /// Name a place for the document and save it there.
    SaveAs,
    /// ⌘+ (or ⌘=), ⌘−, ⌘0: the text set larger or smaller. The host owns the
    /// zoom because it owns the [`Look`](crate::Look) it scales.
    Zoom(ZoomCommand),
    /// ⌘, — the host's settings, if it has any.
    Settings,
}

/// Where the caret is, for a host whose input method does not come through
/// winit — a browser's, through a text field the host keeps beside the caret
/// — in CSS pixels from the window's top left, and the text around it.
#[derive(Clone, Debug, PartialEq)]
pub struct InputArea {
    pub x: f64,
    pub y: f64,
    pub height: f64,
    /// `None` while a composition is open: the field holds that itself.
    pub around: Option<Around>,
}

/// The text as shown around the caret — in the rendered view the words
/// without their markup, a line a row — that a host's text field holds, so
/// that a screen reader reads the document and a keyboard's suggestions see
/// its words: some lines either side of the caret's, with the caret and the
/// selection's anchor as byte offsets into it, and `start`, where it starts
/// in the whole text as shown, the offsets [`Editor::select_shown`] takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Around {
    pub text: String,
    pub start: usize,
    pub caret: usize,
    pub anchor: usize,
}

/// A picture a host loaded itself: see [`Host::picture`].
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub url: String,
    pub size: (f32, f32),
}

/// The editor's host. See the module's doc for what each default does.
///
/// The methods that take an [`Editor`] are called with nothing of the
/// editor's borrowed, so they may read or change the document
/// ([`Editor::doc`]) and repaint. The others may be called while the document
/// is borrowed, and must not call back into the editor.
pub trait Host {
    /// Carry out a command the editor leaves to the host.
    fn request(&self, ed: &Editor, request: Request) {
        let _ = (ed, request);
    }

    /// Put `text`, and `html` when there is a rich flavor, on the clipboard.
    fn write_clipboard(&self, text: &str, html: Option<&str>) -> Result<(), String> {
        system::write(text, html)
    }

    /// What the clipboard holds.
    fn read_clipboard(&self) -> Clipboard {
        system::read()
    }

    /// The caret moved, or the text around it changed: what the editor tells
    /// winit's input method, for a host that brings its own. Called while
    /// nothing of the editor's is borrowed, but must not call back into it.
    fn input_area(&self, area: &InputArea) {
        let _ = area;
    }

    /// ⌃⌘Space: open the system's emoji and symbols picker. Its choice comes
    /// back as an input method commit.
    fn character_palette(&self) {}

    /// Hand a URL, or a file beside the document, to whatever the system
    /// opens it with: an *Open Link* that does not land in this document.
    fn open(&self, what: &str) {
        let _ = what;
    }

    /// A right-click placed the caret: pop up a menu offering what `cx`
    /// says, and send its choices back as the editor's methods
    /// ([`Editor::follow_footnote`], [`Editor::open_link`],
    /// [`Editor::copy_link`]) or as the chords they stand for
    /// ([`Editor::key`]).
    fn context_menu(&self, cx: &Context) {
        let _ = cx;
    }

    /// The host's own reading of a picture, video or audio source in `doc`
    /// — `dest` as written, `![alt](dest)` — answered on the spot: the file on
    /// disk it names. leaf-swift's `onLocateMedia`.
    ///
    /// Asked first. `None` (the default) is no opinion, and the editor's own
    /// reading stands: an absolute path or a `file:` URL as it is, a relative
    /// one against the document's directory, and nothing for a web URL or a
    /// `data:` URI. Answer for what a host spells its own way — a vault's
    /// pages, whose relative paths are the vault's and whose documents may not
    /// be files — or keeps somewhere else. A file that will not read draws as
    /// a chip with the alt text.
    ///
    /// A path, not a URL, because the editor reads the file's header for the
    /// box's size before Blitz decodes it (as a `file://` URL). Asked on every
    /// repaint, for every block, while the document is borrowed; the editor
    /// measures each path once, so an answer only has to be quick. Like
    /// LeafUI, the editor never touches the network: a remote source is the
    /// host's to fetch, and to answer here with the file it fetched to.
    fn locate_media(&self, doc: &Doc, dest: &str) -> Option<PathBuf> {
        let _ = (doc, dest);
        None
    }

    /// A picture the host loaded itself, for `dest` as written in `doc`: the
    /// URL Blitz draws it from (a `data:` URI, or a web address that allows
    /// it) and its natural size ([`picture_size`](crate::picture_size)
    /// measures one). Asked before [`locate_media`](Self::locate_media), on
    /// every repaint, for every medium, while the document is borrowed, so
    /// an answer has to be at hand: a host that has to fetch a picture
    /// answers `None` (the default), fetches it, and repaints when it has it.
    /// The browser's host is one: it has no files.
    fn picture(&self, doc: &Doc, dest: &str) -> Option<Picture> {
        let _ = (doc, dest);
        None
    }

    /// The editor has just repainted `doc`: the moment to bring anything
    /// outside the editor up to date with it, such as a window title.
    fn repainted(&self, doc: &Doc) {
        let _ = doc;
    }

    /// A finger dragged the page sideways far enough to turn it, the way
    /// [`Editor::set_turns`] allowed: `1` forward (dragged leftward), `-1`
    /// back. The host goes to the page before or after.
    fn turn(&self, ed: &Editor, way: i32) {
        let _ = (ed, way);
    }

    /// How tall a card the host draws under a directive's label, in CSS
    /// pixels — an album's pictures, a page's opening words — or `None` (the
    /// default) for none, leaving the label row alone. Asked on every
    /// repaint, for every directive, while the document is borrowed.
    fn directive_height(&self, doc: &Doc, directive: &Directive) -> Option<f32> {
        let _ = (doc, directive);
        None
    }

    /// The card under a directive's label, drawn in the height
    /// [`directive_height`](Self::directive_height) answered: the host's own
    /// markup, which handles its own clicks — calling `stop_propagation` on
    /// one, so that it does not also place the caret.
    fn draw_directive(&self, directive: &Directive) -> Element {
        let _ = directive;
        rsx! {}
    }
}

/// Do a clipboard gesture. Answers whether the document changed, so the
/// caller knows to repaint.
pub fn run(host: &dyn Host, doc: &mut Doc, gesture: Gesture) -> bool {
    match gesture {
        Gesture::Copy => {
            copy(host, doc);
            false
        }
        Gesture::Cut => {
            if copy(host, doc) {
                doc.backspace();
                true
            } else {
                false
            }
        }
        Gesture::Paste => paste(host, doc, true),
        Gesture::PastePlain => paste(host, doc, false),
    }
}

/// Publish the selection; `false` if there was none to publish.
fn copy(host: &dyn Host, doc: &mut Doc) -> bool {
    let Some(text) = doc.selected_text().map(str::to_owned) else {
        return false;
    };
    let html = doc.selection_html();
    if let Err(e) = host.write_clipboard(&text, html.as_deref()) {
        doc.status = Some(format!("copy failed: {e}"));
        return false;
    }
    true
}

fn paste(host: &dyn Host, doc: &mut Doc, rich: bool) -> bool {
    let board = host.read_clipboard();
    if rich
        && let Some(html) = board.html
        && doc.paste_html(&html)
    {
        return true;
    }
    match board.text {
        Some(text) if !text.is_empty() => {
            doc.paste(&text);
            true
        }
        _ => false,
    }
}

/// The default host's clipboard: this process owns the system's outright, so
/// it goes straight there.
#[cfg(feature = "system-clipboard")]
mod system {
    use super::Clipboard;

    pub fn write(text: &str, html: Option<&str>) -> Result<(), String> {
        let mut board = arboard::Clipboard::new().map_err(|_| "no clipboard".to_string())?;
        match html {
            Some(html) => board.set_html(html, Some(text)),
            None => board.set_text(text),
        }
        .map_err(|e| e.to_string())
    }

    pub fn read() -> Clipboard {
        let Ok(mut board) = arboard::Clipboard::new() else {
            return Clipboard::default();
        };
        Clipboard {
            html: board.get().html().ok(),
            text: board.get_text().ok(),
        }
    }
}

/// No clipboard of its own: a host without `system-clipboard` brings one.
#[cfg(not(feature = "system-clipboard"))]
mod system {
    use super::Clipboard;

    pub fn write(_text: &str, _html: Option<&str>) -> Result<(), String> {
        Err("no clipboard".into())
    }

    pub fn read() -> Clipboard {
        Clipboard::default()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use leaf_core::Format;

    use super::*;

    /// A clipboard in memory, so the gestures can be tested without the
    /// system's.
    #[derive(Default)]
    struct Board(RefCell<Clipboard>);

    impl Host for Board {
        fn write_clipboard(&self, text: &str, html: Option<&str>) -> Result<(), String> {
            *self.0.borrow_mut() = Clipboard {
                text: Some(text.into()),
                html: html.map(Into::into),
            };
            Ok(())
        }

        fn read_clipboard(&self) -> Clipboard {
            self.0.borrow().clone()
        }
    }

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.to_string(), Format::Markdown).expect("a document");
        d.build_visual(80);
        d
    }

    #[test]
    fn cut_publishes_both_flavors_and_paste_puts_it_back() {
        let board = Board::default();
        let mut d = doc("one **two** three");
        d.select_all();
        assert!(run(&board, &mut d, Gesture::Cut));
        assert_eq!(d.source, "");
        let held = board.read_clipboard();
        assert_eq!(held.text.as_deref(), Some("one **two** three"));
        assert!(held.html.is_some_and(|h| h.contains("two")));
        assert!(run(&board, &mut d, Gesture::Paste));
        assert!(d.source.contains("two"), "{}", d.source);
    }

    #[test]
    fn copy_with_nothing_selected_leaves_the_clipboard_alone() {
        let board = Board::default();
        let mut d = doc("text");
        assert!(!run(&board, &mut d, Gesture::Copy));
        assert_eq!(board.read_clipboard(), Clipboard::default());
    }

    /// A vault: a leading `/` is its own root, and a document has no file.
    struct Vault(PathBuf);

    impl Host for Vault {
        fn locate_media(&self, _doc: &Doc, dest: &str) -> Option<PathBuf> {
            dest.strip_prefix('/').map(|rest| self.0.join(rest))
        }
    }

    #[test]
    fn the_hosts_reading_of_a_source_comes_first_and_none_leaves_the_editors() {
        let vault = Vault(PathBuf::from("/vault"));
        let mut d = doc("text");
        d.path = PathBuf::from("/notes/page.md");
        let locate = |dest| crate::blocks::locate(&vault, &d, dest);
        assert_eq!(
            locate("/pics/cat.png"),
            Some(PathBuf::from("/vault/pics/cat.png"))
        );
        // No opinion: relative to the document, as with no host at all.
        assert_eq!(locate("cat.png"), Some(PathBuf::from("/notes/cat.png")));
        assert_eq!(
            crate::blocks::locate(&Board::default(), &d, "/pics/cat.png"),
            Some(PathBuf::from("/pics/cat.png"))
        );
    }

    /// A host with no files, which loaded the picture itself.
    struct Loaded;

    impl Host for Loaded {
        fn picture(&self, _doc: &Doc, dest: &str) -> Option<Picture> {
            (dest == "cat.png").then(|| Picture {
                url: "data:image/png;base64,AAAA".into(),
                size: (30.0, 20.0),
            })
        }
    }

    #[test]
    fn a_picture_the_host_loaded_draws_from_its_url() {
        let mut d = Doc::from_source(
            "![a cat](cat.png)\n\n![a dog](dog.png)\n".into(),
            Format::Markdown,
        )
        .expect("a document");
        d.build_visual_unwrapped();
        let block = |i: usize| {
            crate::media::block(
                &d.vmap.media[i],
                &d.vmap.rows,
                &|dest| crate::blocks::find_still(&Loaded, &d, dest),
                0,
                None,
                &mut crate::text::Fonts::new(crate::text::Type::default()),
                600.0,
                |_| None,
            )
            .unwrap()
        };
        let still = block(0).still.expect("the host's picture");
        assert_eq!(still.url, "data:image/png;base64,AAAA");
        assert_eq!(still.size, (30.0, 20.0));
        assert!(block(1).still.is_none(), "nothing loaded is a chip");
    }

    #[test]
    fn a_picture_in_an_untitled_document_draws_from_where_the_host_says() {
        let dir = std::env::temp_dir().join(format!("agent-leaf-locate-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pics")).unwrap();
        image::RgbImage::new(30, 20)
            .save(dir.join("pics/cat.png"))
            .unwrap();
        let vault = Vault(dir.clone());
        let mut d = Doc::from_source("![a cat](/pics/cat.png)\n".into(), Format::Markdown)
            .expect("a document");
        d.build_visual_unwrapped();
        assert!(d.is_untitled());
        let block = crate::media::block(
            &d.vmap.media[0],
            &d.vmap.rows,
            &|dest| crate::blocks::find_still(&vault, &d, dest),
            0,
            None,
            &mut crate::text::Fonts::new(crate::text::Type::default()),
            600.0,
            |_| None,
        );
        std::fs::remove_dir_all(&dir).unwrap();
        let still = block.unwrap().still.expect("the picture loaded");
        assert_eq!(still.size, (30.0, 20.0));
    }

    #[test]
    fn a_plain_paste_takes_the_text_and_never_the_html() {
        let board = Board::default();
        board
            .write_clipboard("plain", Some("<b>rich</b>"))
            .expect("a write");
        let mut d = doc("");
        assert!(run(&board, &mut d, Gesture::PastePlain));
        assert_eq!(d.source, "plain");
    }
}
