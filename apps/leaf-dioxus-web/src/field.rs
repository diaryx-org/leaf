//! The page's text field: how the browser's keyboard reaches the editor.
//!
//! winit's canvas hears keys, but not the input method — no dead key, no
//! phone keyboard, no dictation, no Japanese — and it cancels every key's
//! default, so the browser never fires `paste` either. A text field does
//! both, so the page keeps one beside the caret, invisible, and keeps the
//! focus in it, and turns what it hears into what the editor takes from
//! winit on the Mac:
//!
//! - **Keys** (`keydown`): the editor's bindings — arrows, Return, ⌘B — go to
//!   [`Editor::key_taken`], and their default is cancelled only when the
//!   editor took them, so ⌘R and ⌘L still reach the browser. A key that types
//!   is left to the field, whose `beforeinput` says what it typed after the
//!   layout and any dead key have had their say.
//! - **Input** (`beforeinput`): what was typed, or what a phone's keyboard
//!   or an autocorrection did — insert, delete a word, undo — as the
//!   editor's own keys, every default cancelled so the field holds what the
//!   page put there and nothing else.
//! - **Composition** (`compositionupdate`, `compositionend`): the input
//!   method's marked text and its commit, as winit's `Ime` events, which
//!   `ime.rs` writes into the document as they come. A composition edits the
//!   field and cannot be cancelled; the field is rewritten once it ends.
//! - **The clipboard** (`copy`, `cut`, `paste`): the event's own data, both
//!   flavours, without asking permission. Copy and Cut fill it from the
//!   document's selection; a paste from anywhere puts what it carries on the
//!   page's clipboard and has the editor paste it with ⌘V, as it would the
//!   Mac's.
//!
//! - **Dropping** (`drop` on the page): text or HTML from another page or
//!   app goes in where it lands, as a paste would; a picture file goes into
//!   the page's memory ([`Pictures::keep`]) and into the document as a
//!   picture of its own, on its own line; a document file is opened.
//!
//! The field holds the document's text as shown around the caret — its
//! words without their markup, a line a row — with the document's selection
//! over the same text ([`InputArea`]). A screen reader reads the document
//! there, and follows the caret as it moves, since the field's selection
//! moves with it; a keyboard's suggestions and an autocorrection see the
//! words around the caret. When the browser moves the field's selection
//! itself before an input — an autocorrection names the word it replaces
//! that way — the document's selection follows it first
//! ([`Editor::select_shown`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::str::FromStr;

use dioxus::html::{Code, Key, Modifiers, NamedKey};
use dioxus_native::winit::event::{Ime, WindowEvent};
use leaf_dioxus::{Around, Clipboard, Editor, InputArea};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    ClipboardEvent, CompositionEvent, Document, DragEvent, FocusEvent, HtmlTextAreaElement,
    InputEvent, KeyboardEvent,
};

use crate::files::Files;
use crate::pictures::Pictures;
use crate::web::Ed;

/// The field, and what the page's clipboard holds.
pub struct Field {
    area: HtmlTextAreaElement,
    /// What the field was last given: the line, where it starts in the
    /// source, and the selection in it, as the editor reported them.
    mirror: RefCell<Mirror>,
    /// The page's clipboard: what the last Copy or Cut put there, or what
    /// the last paste carried in.
    pub board: RefCell<Clipboard>,
    /// A Copy or Cut is under way inside the browser's `copy` or `cut`
    /// event, which carries the data itself.
    in_clipboard_event: Cell<bool>,
    /// The paste the next `paste` event is: ⇧ held for ⇧⌘V.
    plain: Cell<bool>,
}

#[derive(Default)]
struct Mirror {
    /// Where `text` starts in the whole text as shown, in bytes.
    base: usize,
    text: String,
    /// The caret and the anchor in `text`, in UTF-16 code units, the
    /// field's own unit.
    caret: u32,
    anchor: u32,
}

impl Field {
    /// The field, added to the page. Nothing reaches the editor until
    /// [`Field::listen`].
    pub fn new() -> Rc<Self> {
        let doc = document();
        let area: HtmlTextAreaElement = doc
            .create_element("textarea")
            .expect("a textarea")
            .unchecked_into();
        for (name, value) in [
            ("aria-label", "Document"),
            ("aria-multiline", "true"),
            ("autocapitalize", "sentences"),
            ("autocomplete", "off"),
            ("spellcheck", "true"),
            ("wrap", "off"),
        ] {
            let _ = area.set_attribute(name, value);
        }
        // Invisible, out of the pointer's way, and one pixel wide so that its
        // own caret — where the browser opens an input method's candidates
        // — is wherever the field is put. 16px, or iOS zooms the page when
        // the field takes the focus.
        let _ = area.set_attribute(
            "style",
            "position: fixed; left: 0; top: 0; width: 1px; height: 1em; \
             margin: 0; padding: 0; border: 0; outline: none; resize: none; \
             overflow: hidden; white-space: pre; font-size: 16px; \
             opacity: 0; color: transparent; background: transparent; \
             caret-color: transparent; pointer-events: none; z-index: 1;",
        );
        if let Some(body) = doc.body() {
            let _ = body.append_child(&area);
        }
        Rc::new(Self {
            area,
            mirror: RefCell::default(),
            board: RefCell::default(),
            in_clipboard_event: Cell::new(false),
            plain: Cell::new(false),
        })
    }

    /// Whether a Copy or Cut is being answered inside the browser's own
    /// event, which carries the data: the system's clipboard needs nothing
    /// more then.
    pub fn in_clipboard_event(&self) -> bool {
        self.in_clipboard_event.get()
    }

    /// Put the field at the caret, holding the text around it.
    pub fn place(&self, at: &InputArea) {
        let style = self.area.style();
        let _ = style.set_property("left", &format!("{}px", at.x));
        let _ = style.set_property("top", &format!("{}px", at.y));
        let _ = style.set_property("height", &format!("{}px", at.height.max(1.0)));
        // While a composition is open the field is the input method's.
        let Some(Around {
            text,
            start,
            caret,
            anchor,
        }) = &at.around
        else {
            return;
        };
        let mut m = self.mirror.borrow_mut();
        m.base = *start;
        if m.text != *text || self.area.value() != *text {
            self.area.set_value(text);
            m.text.clone_from(text);
        }
        m.caret = utf16(text, *caret);
        m.anchor = utf16(text, *anchor);
        let (start, end) = (m.caret.min(m.anchor), m.caret.max(m.anchor));
        let backward = m.caret < m.anchor;
        let now = (
            self.area.selection_start().ok().flatten(),
            self.area.selection_end().ok().flatten(),
        );
        if now != (Some(start), Some(end)) {
            let _ = self.area.set_selection_range_with_direction(
                start,
                end,
                if backward { "backward" } else { "forward" },
            );
        }
    }

    /// Take the focus, without scrolling the page to the field.
    pub fn focus(&self) {
        let opts = web_sys::FocusOptions::new();
        opts.set_prevent_scroll(true);
        let _ = self.area.focus_with_options(&opts);
    }

    /// Send the field's events to the editor, and keep the focus here when
    /// the canvas takes it.
    pub fn listen(self: &Rc<Self>, ed: Rc<Ed>, pictures: Rc<Pictures>, files: Rc<Files>) {
        let cx = Rc::new(Cx {
            field: self.clone(),
            ed,
            pictures,
            files,
        });
        let on = |name: &str, f: Box<dyn FnMut(web_sys::Event)>| {
            let f = Closure::<dyn FnMut(web_sys::Event)>::wrap(f);
            let _ = self
                .area
                .add_event_listener_with_callback(name, f.as_ref().unchecked_ref());
            f.forget();
        };
        let c = cx.clone();
        on("keydown", Box::new(move |e| c.keydown(e.unchecked_ref())));
        let c = cx.clone();
        on(
            "beforeinput",
            Box::new(move |e| c.beforeinput(e.unchecked_ref())),
        );
        let c = cx.clone();
        on(
            "compositionupdate",
            Box::new(move |e| c.composition(e.unchecked_ref(), false)),
        );
        let c = cx.clone();
        on(
            "compositionend",
            Box::new(move |e| c.composition(e.unchecked_ref(), true)),
        );
        let c = cx.clone();
        on("copy", Box::new(move |e| c.copy(e.unchecked_ref(), false)));
        let c = cx.clone();
        on("cut", Box::new(move |e| c.copy(e.unchecked_ref(), true)));
        let c = cx.clone();
        on("paste", Box::new(move |e| c.paste(e.unchecked_ref())));

        // A drop anywhere on the page: what is over the canvas is the
        // editor's, and the browser opening a dropped file in the tab is
        // never wanted.
        let doc = document();
        let over = Closure::<dyn FnMut(DragEvent)>::new(|e: DragEvent| {
            e.prevent_default();
            if let Some(d) = e.data_transfer() {
                d.set_drop_effect("copy");
            }
        });
        let _ = doc.add_event_listener_with_callback("dragover", over.as_ref().unchecked_ref());
        over.forget();
        let c = cx.clone();
        let dropped = Closure::<dyn FnMut(DragEvent)>::new(move |e: DragEvent| {
            e.prevent_default();
            c.drop(&e);
        });
        let _ = doc.add_event_listener_with_callback("drop", dropped.as_ref().unchecked_ref());
        dropped.forget();

        // winit focuses the canvas on every press; the focus comes straight
        // back here, so the next key is the field's. Once this listener has
        // returned: taking the focus fires `focusin` again, into it.
        let field = self.clone();
        let take = Closure::<dyn Fn()>::new(move || field.focus());
        let refocus = Closure::<dyn FnMut(FocusEvent)>::new(move |e: FocusEvent| {
            let canvas = e
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlCanvasElement>().ok());
            if let (Some(_), Some(window)) = (canvas, web_sys::window()) {
                window.queue_microtask(take.as_ref().unchecked_ref());
            }
        });
        let _ = document()
            .add_event_listener_with_callback("focusin", refocus.as_ref().unchecked_ref());
        refocus.forget();
        self.focus();
    }
}

/// What a listener needs: the field and the editor; and the pictures and
/// files, for what is dropped and for ⌘O.
struct Cx {
    field: Rc<Field>,
    ed: Rc<Ed>,
    pictures: Rc<Pictures>,
    files: Rc<Files>,
}

impl Cx {
    fn run<R>(&self, f: impl FnOnce(&Editor) -> R) -> R {
        self.ed.run(f)
    }

    fn chord(&self, key: Key, code: Code, mods: Modifiers) -> bool {
        self.run(|ed| ed.key_taken(key, code, mods))
    }

    fn named(&self, key: NamedKey, code: Code, mods: Modifiers) -> bool {
        self.chord(Key::Named(key), code, mods)
    }

    fn keydown(&self, e: &KeyboardEvent) {
        // An input method's keystroke is its own, and arrives as composition.
        if e.is_composing() || e.key_code() == 229 {
            return;
        }
        let name = e.key();
        if name == "Dead" || name == "Process" || name == "Unidentified" {
            return;
        }
        let key = Key::from_str(&name).unwrap_or(Key::Named(NamedKey::Unidentified));
        let code = Code::from_str(&e.code()).unwrap_or(Code::Unidentified);
        let mods = modifiers(e);
        let command = mods.contains(command_key());
        let emacs = mac() && mods.contains(Modifiers::CONTROL);
        // AltGr is Ctrl and Alt together off the Mac, and types a character.
        let alt_gr = !mac() && e.ctrl_key() && e.alt_key();
        if let Key::Character(_) = key
            && (!(command || emacs) || alt_gr)
        {
            // Typed: the field's `beforeinput` says what it typed.
            return;
        }
        // ⌘O and ⇧⌘S: the page's open and Save As, not the browser's.
        if command && !e.alt_key() && code == Code::KeyO && !e.shift_key() {
            e.prevent_default();
            self.files.open();
            return;
        }
        if command && !e.alt_key() && code == Code::KeyS && e.shift_key() {
            e.prevent_default();
            self.files.save(true);
            return;
        }
        // The clipboard's chords are the browser's, whose `copy`, `cut` and
        // `paste` carry the data.
        if command && !e.alt_key() && matches!(code, Code::KeyC | Code::KeyX | Code::KeyV) {
            self.field.plain.set(e.shift_key());
            return;
        }
        if self.chord(key, code, mods) {
            e.prevent_default();
        }
    }

    fn beforeinput(&self, e: &InputEvent) {
        let kind = e.input_type();
        // A composition's own steps: `compositionupdate` and
        // `compositionend` carry them, and they cannot be cancelled.
        if e.is_composing()
            || matches!(
                kind.as_str(),
                "insertCompositionText" | "deleteCompositionText" | "insertFromComposition"
            )
        {
            return;
        }
        e.prevent_default();
        // The clipboard and dragging have events of their own.
        if matches!(
            kind.as_str(),
            "insertFromPaste"
                | "insertFromPasteAsQuotation"
                | "deleteByCut"
                | "insertFromDrop"
                | "deleteByDrag"
        ) {
            return;
        }
        self.follow_selection();
        let none = Modifiers::empty();
        let cmd = command_key();
        match kind.as_str() {
            "insertText" | "insertReplacementText" => {
                let text = e
                    .data()
                    .or_else(|| {
                        e.data_transfer()
                            .and_then(|d| d.get_data("text/plain").ok())
                    })
                    .unwrap_or_default();
                match text.as_str() {
                    "" => {}
                    "\n" | "\r" | "\r\n" => {
                        self.named(NamedKey::Enter, Code::Enter, none);
                    }
                    _ => {
                        self.chord(Key::Character(text), Code::Unidentified, none);
                    }
                }
            }
            "insertParagraph" => {
                self.named(NamedKey::Enter, Code::Enter, none);
            }
            "insertLineBreak" => {
                self.named(NamedKey::Enter, Code::Enter, Modifiers::SHIFT);
            }
            "deleteContentBackward" => {
                self.named(NamedKey::Backspace, Code::Backspace, none);
            }
            "deleteContentForward" => {
                self.named(NamedKey::Delete, Code::Delete, none);
            }
            "deleteWordBackward" => {
                self.named(NamedKey::Backspace, Code::Backspace, Modifiers::ALT);
            }
            "deleteWordForward" => {
                self.named(NamedKey::Delete, Code::Delete, Modifiers::ALT);
            }
            "deleteSoftLineBackward" | "deleteHardLineBackward" => {
                self.named(NamedKey::Backspace, Code::Backspace, cmd);
            }
            "deleteSoftLineForward" | "deleteHardLineForward" => self.run(|ed| {
                ed.doc().delete_to_line_end();
                ed.refresh();
                ed.ime_area();
            }),
            "historyUndo" => {
                self.chord(Key::Character("z".into()), Code::KeyZ, cmd);
            }
            "historyRedo" => {
                self.chord(
                    Key::Character("z".into()),
                    Code::KeyZ,
                    cmd | Modifiers::SHIFT,
                );
            }
            "formatBold" => {
                self.chord(Key::Character("b".into()), Code::KeyB, cmd);
            }
            "formatItalic" => {
                self.chord(Key::Character("i".into()), Code::KeyI, cmd);
            }
            "formatUnderline" => {
                self.chord(Key::Character("u".into()), Code::KeyU, cmd);
            }
            _ => {}
        }
    }

    /// The browser moved the field's selection before an input — an
    /// autocorrection selects the word it replaces, a phone's keyboard the
    /// letters it deletes — so the document's selection moves to match.
    fn follow_selection(&self) {
        let area = &self.field.area;
        let (Some(start), Some(end)) = (
            area.selection_start().ok().flatten(),
            area.selection_end().ok().flatten(),
        ) else {
            return;
        };
        let m = self.field.mirror.borrow();
        if (start, end) == (m.caret.min(m.anchor), m.caret.max(m.anchor)) || area.value() != m.text
        {
            return;
        }
        let (a, b) = (m.base + bytes(&m.text, start), m.base + bytes(&m.text, end));
        drop(m);
        self.run(|ed| ed.select_shown(a, b));
    }

    fn composition(&self, e: &CompositionEvent, end: bool) {
        let text = e.data().unwrap_or_default();
        self.run(|ed| {
            let len = text.len();
            ed.window_event(&WindowEvent::Ime(Ime::Preedit(
                if end { String::new() } else { text.clone() },
                (!end).then_some((len, len)),
            )));
            if end && !text.is_empty() {
                ed.window_event(&WindowEvent::Ime(Ime::Commit(text)));
            }
        });
    }

    /// Something dropped on the page, at the point it was dropped: picture
    /// files, or else text in its richest flavour. The caret goes there
    /// first; a drop off the text — on a table, a picture, the margin —
    /// lands at the caret.
    fn drop(self: &Rc<Self>, e: &DragEvent) {
        let Some(data) = e.data_transfer() else {
            return;
        };
        let (x, y) = (e.client_x() as f64, e.client_y() as f64);
        let at = self.run(|ed| ed.offset_at_point(x, y));
        self.field.focus();
        let files: Vec<web_sys::File> = data
            .files()
            .map(|list| (0..list.length()).filter_map(|i| list.get(i)).collect())
            .unwrap_or_default();
        // A document dropped is opened, in this page's place.
        if let Some(file) = files
            .iter()
            .find(|f| crate::files::format_of(&f.name()).is_some())
        {
            self.files.open_file(file.clone());
            return;
        }
        let pictures: Vec<_> = files
            .into_iter()
            .filter(|f| f.type_().starts_with("image/"))
            .collect();
        if !pictures.is_empty() {
            let cx = self.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut names = Vec::new();
                for file in pictures {
                    let Ok(buffer) = JsFuture::from(file.array_buffer()).await else {
                        continue;
                    };
                    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                    names.push(cx.pictures.keep(&file.name(), bytes));
                }
                cx.run(|ed| {
                    {
                        let mut d = ed.doc();
                        if let Some(at) = at {
                            d.place_caret(at, false);
                        }
                        let lines = picture_lines(&d.source, d.caret, &names);
                        d.paste(&lines);
                    }
                    ed.refresh();
                    ed.ime_area();
                });
            });
            return;
        }
        let get = |kind: &str| data.get_data(kind).ok().filter(|s| !s.is_empty());
        let board = Clipboard {
            text: get("text/plain"),
            html: get("text/html"),
        };
        if board.text.is_none() && board.html.is_none() {
            return;
        }
        *self.field.board.borrow_mut() = board;
        if let Some(at) = at {
            self.run(|ed| ed.doc().place_caret(at, false));
        }
        self.chord(Key::Character("v".into()), Code::KeyV, command_key());
    }

    /// Copy or Cut: the document's selection, in both flavours, into the
    /// event's data. Nothing selected leaves the event to the browser.
    fn copy(&self, e: &ClipboardEvent, cut: bool) {
        let (Some(data), mods) = (e.clipboard_data(), command_key()) else {
            return;
        };
        let (text, html) = {
            let doc = self.ed.editor().shared_doc();
            let mut d = doc.borrow_mut();
            let Some(text) = d.selected_text().map(str::to_owned) else {
                return;
            };
            (text, d.selection_html())
        };
        e.prevent_default();
        let _ = data.set_data("text/plain", &text);
        if let Some(html) = &html {
            let _ = data.set_data("text/html", html);
        }
        // The editor's own gesture, for the page's clipboard and, for Cut,
        // the deletion; the host leaves the system's clipboard alone while
        // this flag is up.
        self.field.in_clipboard_event.set(true);
        let key = if cut {
            ("x", Code::KeyX)
        } else {
            ("c", Code::KeyC)
        };
        self.chord(Key::Character(key.0.into()), key.1, mods);
        self.field.in_clipboard_event.set(false);
    }

    /// A paste from anywhere: what it carries onto the page's clipboard,
    /// then the editor's own paste of it.
    fn paste(&self, e: &ClipboardEvent) {
        let Some(data) = e.clipboard_data() else {
            return;
        };
        e.prevent_default();
        let get = |kind: &str| data.get_data(kind).ok().filter(|s| !s.is_empty());
        *self.field.board.borrow_mut() = Clipboard {
            text: get("text/plain"),
            html: get("text/html"),
        };
        let mods = match self.field.plain.replace(false) {
            true => command_key() | Modifiers::SHIFT,
            false => command_key(),
        };
        self.chord(Key::Character("v".into()), Code::KeyV, mods);
    }
}

/// Pictures named `names`, as the source to insert at `caret` in `source`:
/// each a paragraph of its own, set apart from the text around the caret.
fn picture_lines(source: &str, caret: usize, names: &[String]) -> String {
    let body = names
        .iter()
        .map(|n| {
            let alt = n.rsplit_once('.').map_or(n.as_str(), |(s, _)| s);
            format!("![{}]({n})", alt.replace('-', " "))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let before = &source[..caret];
    let after = &source[caret..];
    let lead = match () {
        _ if before.is_empty() || before.ends_with("\n\n") => "",
        _ if before.ends_with('\n') => "\n",
        _ => "\n\n",
    };
    let tail = match () {
        _ if after.starts_with("\n\n") => "",
        _ if after.starts_with('\n') || after.is_empty() => "\n",
        _ => "\n\n",
    };
    format!("{lead}{body}{tail}")
}

fn document() -> Document {
    web_sys::window()
        .and_then(|w| w.document())
        .expect("a document")
}

/// A byte offset into `text` in UTF-16 code units.
fn utf16(text: &str, at: usize) -> u32 {
    text.get(..at)
        .map_or(0, |s| s.encode_utf16().count() as u32)
}

/// A UTF-16 offset into `text` in bytes.
fn bytes(text: &str, at: u32) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= at {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    text.len()
}

fn modifiers(e: &KeyboardEvent) -> Modifiers {
    let mut m = Modifiers::empty();
    m.set(Modifiers::SHIFT, e.shift_key());
    m.set(Modifiers::CONTROL, e.ctrl_key());
    m.set(Modifiers::ALT, e.alt_key());
    m.set(Modifiers::META, e.meta_key());
    m
}

/// Whether the browser runs on a Mac, an iPhone or an iPad, whose command key
/// is ⌘ — the widget's own rule (`input.rs`).
fn mac() -> bool {
    thread_local! {
        static MAC: bool = web_sys::window()
            .and_then(|w| w.navigator().user_agent().ok())
            .is_some_and(|ua| ["Macintosh", "iPhone", "iPad"].iter().any(|p| ua.contains(p)));
    }
    MAC.with(|m| *m)
}

fn command_key() -> Modifiers {
    if mac() {
        Modifiers::META
    } else {
        Modifiers::CONTROL
    }
}
