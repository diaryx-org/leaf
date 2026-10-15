//! Where input arrives: keys, the input method, and the menu bar's commands.
//!
//! The keys and the input method come from winit, not from Blitz — see
//! `keys.rs` and `ime.rs` for why — and the menu bar's items are key chords
//! replayed (`menu.rs`), so a command is a key binding in `input.rs` however
//! it is chosen. This file routes each to the find bar, the composer, the
//! document, or a file dialog.

use dioxus::html::{Code, Key, Modifiers};
use dioxus::prelude::*;
use dioxus_native::winit::dpi::{LogicalPosition, LogicalSize};
use dioxus_native::winit::event::Ime;
use dioxus_native::winit::window::{
    ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, ImeSurroundingText,
};

use crate::editor::Editor;
use crate::file::FileAction;
use crate::find::{Focus, Outcome};
use crate::input::{Action, FindCommand, Host, apply_key};
use crate::menu::Command;
use crate::trace::trace;
use crate::viewport::LINE;
use crate::{host, ime};

impl Editor {
    /// A key, from the window or replayed by the menu bar.
    pub fn key(&self, key: Key, code: Code, mods: Modifiers) {
        let st = &*self.st;
        st.composer.borrow_mut().settle(&mut st.doc.borrow_mut());
        // Typing goes to the find bar while it has focus; a key it does not
        // take (⌘G, ⌘Z, ⌘S…) falls through to the document's.
        let in_bar = st.find.borrow().open && st.find.borrow().focus != Focus::Doc;
        if in_bar {
            let o = st.find.borrow_mut().key(&key, mods);
            if o != Outcome::Ignored {
                trace(|| format!("find key {key:?} -> {o:?}"));
                self.run_find(o);
                return;
            }
        }
        let action = apply_key(&mut st.doc.borrow_mut(), &key, code, mods);
        trace(|| format!("key {key:?} {code:?} {mods:?} -> {action:?}"));
        if !matches!(action, Action::Vertical { .. } | Action::Page { .. } | Action::Ignored) {
            st.goal.set(None);
        }
        let changed = match action {
            Action::Ignored => return,
            Action::Handled => true,
            // ⌘S on a document with no file yet is a Save As, and only the
            // editor has a dialog to ask with.
            Action::Host(Host::Save) if st.doc.borrow().is_untitled() => {
                self.file(FileAction::SaveAs);
                return;
            }
            Action::Host(h) => host::run(&mut st.doc.borrow_mut(), h),
            Action::Vertical { down, extend } => {
                self.vertical(down, extend);
                true
            }
            Action::Page { down, extend } => {
                self.page(down, extend);
                true
            }
            Action::LineEdge { end, extend } => {
                self.line_edge(end, extend);
                true
            }
            Action::Find(cmd) => {
                self.find_command(cmd);
                return;
            }
        };
        if changed {
            self.refresh();
        }
    }

    /// ⌘F, ⌥⌘F, ⌘G, ⇧⌘G.
    fn find_command(&self, cmd: FindCommand) {
        let st = &*self.st;
        let empty = st.find.borrow().query.text.is_empty();
        let o = match cmd {
            FindCommand::Open { replace } => {
                st.find.borrow_mut().show(&st.doc.borrow(), replace);
                Outcome::Searched
            }
            // ⌘G with nothing to look for opens the bar instead.
            _ if empty => {
                st.find.borrow_mut().show(&st.doc.borrow(), false);
                Outcome::Edited
            }
            FindCommand::Next => Outcome::Step { forward: true },
            FindCommand::Previous => Outcome::Step { forward: false },
        };
        self.run_find(o);
    }

    /// A menu bar item.
    pub fn command(&self, c: Command) {
        trace(|| format!("menu {c:?}"));
        match c {
            Command::Chord(key, code, mods) => {
                self.key(key, code, mods);
                self.ime_area();
            }
            Command::New => self.file(FileAction::New),
            Command::Open => self.file(FileAction::Open),
            Command::SaveAs => self.file(FileAction::SaveAs),
        }
    }

    /// The input method: a composition's steps, its commit, or text around
    /// the caret to delete first (press-and-hold). See `ime.rs`.
    pub fn ime(&self, ime: &Ime) {
        let st = &*self.st;
        trace(|| format!("ime {ime:?}"));
        // An accent or an emoji committed while the find bar has focus is the
        // query's. A composition's preedit is not drawn there.
        if st.find.borrow().open && st.find.borrow().focus != Focus::Doc {
            if let Ime::Commit(text) = ime {
                let o = st.find.borrow_mut().commit(text);
                self.run_find(o);
            }
            return;
        }
        {
            let mut c = st.composer.borrow_mut();
            let mut d = st.doc.borrow_mut();
            match ime {
                Ime::Preedit(text, cursor) => c.preedit(&mut d, text, *cursor),
                Ime::Commit(text) => c.commit(&mut d, text),
                Ime::DeleteSurrounding { before_bytes, after_bytes } => {
                    c.delete_surrounding(&mut d, *before_bytes, *after_bytes)
                }
                Ime::Disabled => c.settle(&mut d),
                _ => return,
            }
        }
        self.refresh();
        self.ime_area();
    }

    /// Allow the input method on the window, once. Surrounding text needs
    /// the patched winit-appkit to be heard on macOS (see
    /// `patches/winit-appkit/PATCH.md`); winit asks for some with the
    /// capability, so an empty line to start.
    pub fn enable_ime(&self) {
        let request = ImeEnableRequest::new(
            ImeCapabilities::new().with_cursor_area().with_surrounding_text(),
            ImeRequestData::default()
                .with_cursor_area(LogicalPosition::new(0.0, 0.0).into(), LogicalSize::new(2.0, LINE).into())
                .with_surrounding_text(ImeSurroundingText::new(String::new(), 0, 0).expect("an empty excerpt")),
        );
        if let Some(request) = request {
            let _ = self.st.window.request_ime_update(ImeRequest::Enable(request));
        }
    }

    /// Tell the input method where the caret is, so its candidate window and
    /// the accent menu open beside it, and what text surrounds it, so
    /// press-and-hold can see the letter it offers accents for.
    pub fn ime_area(&self) {
        let Some((row, line, x)) = self.caret_at() else { return };
        let lays = self.sig.lays.peek();
        let Some(l) = lays.get(row).and_then(|l| l.0.lines.get(line)) else { return };
        let y = self.sig.layout.peek().top(row) - *self.sig.top.peek() + l.top as f64 + self.sig.bar.peek().height();
        let mut data = ImeRequestData::default().with_cursor_area(
            LogicalPosition::new(crate::row::PAD + x as f64, y).into(),
            LogicalSize::new(2.0, l.height as f64).into(),
        );
        let around = ime::surrounding(&self.st.doc.borrow(), &self.st.composer.borrow());
        if let Some(text) = around.and_then(|(t, c, a)| ImeSurroundingText::new(t, c, a).ok()) {
            data = data.with_surrounding_text(text);
        }
        let _ = self.st.window.request_ime_update(ImeRequest::Update(data));
    }
}
