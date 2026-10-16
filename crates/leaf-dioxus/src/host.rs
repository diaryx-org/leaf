//! What the editor asks of whoever embeds it: the [`Host`].
//!
//! The editor owns the document, its rows and every key binding. What it does
//! not own is the world around it — the clipboard, the system's handler for a
//! URL, a native context menu, the window's title, and what Save means — so
//! it asks. Every method has a default, so `struct Mine; impl Host for Mine {}`
//! is a host: the system clipboard (with `system-clipboard`, the default),
//! links and context menus that go nowhere, and Save, Save As, the zoom and
//! the settings ignored. `apps/leaf-dioxus` answers all of them.
//!
//! The clipboard gestures themselves — what Copy, Cut and Paste do to the
//! document — are here, over the host's raw clipboard. Both flavors travel, as
//! leaf-swift publishes them: the source slice as `text/plain`, and
//! [`Doc::selection_html`] as `text/html`, so a paste into Mail or Docs keeps
//! its formatting. A paste prefers the HTML and falls back to the text, which
//! is [`Doc::paste_html`]'s own contract. A webview would need the `paste`
//! event's `DataTransfer` for these; native has no such restriction.

use leaf_core::Doc;

use crate::editor::Editor;
use crate::input::{Gesture, ZoomCommand};
use crate::link::Context;

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

    /// The editor has just repainted `doc`: the moment to bring anything
    /// outside the editor up to date with it, such as a window title.
    fn repainted(&self, doc: &Doc) {
        let _ = doc;
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
