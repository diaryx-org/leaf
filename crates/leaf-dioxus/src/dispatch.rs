//! Where input arrives: keys, the input method, and the window's other events.
//!
//! The keys and the input method come from winit, not from Blitz — see
//! `keys.rs` and `ime.rs` for why — and a host's menu items are key chords
//! replayed (as `apps/leaf-dioxus`'s `menus.rs` does), so a command is a key
//! binding in `input.rs` however it is chosen. This file routes each to the
//! find bar, the composer, the document, or the host.

use std::rc::Rc;

use dioxus::html::{Code, Key, Modifiers};
use dioxus::prelude::*;
use dioxus_native::winit::dpi::{LogicalPosition, LogicalSize};
use dioxus_native::winit::event::{ButtonSource, Ime, PointerSource, WindowEvent};
use dioxus_native::winit::window::{
    ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, ImeSurroundingText,
};

use crate::editor::Editor;
use crate::find::{Focus, Outcome};
use crate::host::InputArea;
use crate::input::{Action, FindCommand, TableCommand, apply_key, apply_table};
use crate::trace::trace;
use crate::viewport::LINE;
use crate::{host, ime};

impl Editor {
    /// A key, from the window ([`crate::pressed`] makes these from winit's)
    /// or replayed by a menu item; then the input method told where the caret
    /// went.
    pub fn key(&self, key: Key, code: Code, mods: Modifiers) {
        self.key_taken(key, code, mods);
    }

    /// [`Self::key`], answering whether the key was one of the editor's — a
    /// browser host lets the page have the rest (⌘R, ⌘L, F5).
    pub fn key_taken(&self, key: Key, code: Code, mods: Modifiers) -> bool {
        let taken = self.press(key, code, mods);
        self.ime_area();
        taken
    }

    fn press(&self, key: Key, code: Code, mods: Modifiers) -> bool {
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
                return true;
            }
        }
        // Not drawn, a letter typed leaves core's map to the next key that
        // moves by it (`Editor::set_drawn`); any other key has it first.
        let typed = matches!(key, Key::Character(_))
            && !mods.intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT);
        if !typed {
            self.ensure_map();
        }
        let t = std::time::Instant::now();
        let action = apply_key(&mut st.doc.borrow_mut(), &key, code, mods);
        trace(|| {
            format!(
                "key {key:?} {code:?} {mods:?} -> {action:?} in {:.1?}",
                t.elapsed()
            )
        });
        if !matches!(
            action,
            Action::Vertical { .. } | Action::Page { .. } | Action::Ignored
        ) {
            st.goal.set(None);
        }
        let changed = match action {
            Action::Ignored => return false,
            Action::Handled => true,
            Action::Gesture(g) => host::run(&*st.host, &mut st.doc.borrow_mut(), g),
            // The host repaints, if what it did calls for it.
            Action::Request(r) => {
                st.host.request(self, r);
                return true;
            }
            Action::CharacterPalette => {
                st.host.character_palette();
                return true;
            }
            Action::Vertical { down, extend } => {
                match self.host_lines() {
                    Some(lines) => {
                        let mut d = st.doc.borrow_mut();
                        // A table's cells are core's to move between, as
                        // they are in the rows.
                        if extend || !d.caret_in_table() || !d.cell_move_vertical(down) {
                            lines.vertical(&mut d, down, extend, &st.goal);
                        }
                    }
                    None => self.vertical(down, extend),
                }
                true
            }
            Action::Page { down, extend } => {
                match self.host_lines() {
                    Some(lines) => lines.page(&mut st.doc.borrow_mut(), down, extend, &st.goal),
                    None => self.page(down, extend),
                }
                true
            }
            Action::LineEdge { end, extend } => {
                match self.host_lines() {
                    Some(lines) => lines.line_edge(&mut st.doc.borrow_mut(), end, extend),
                    None => self.line_edge(end, extend),
                }
                true
            }
            Action::Find(cmd) => {
                self.find_command(cmd);
                return true;
            }
        };
        if changed {
            self.refresh();
        }
        true
    }

    /// The host's lines, while it has given the editor some.
    fn host_lines(&self) -> Option<Rc<dyn crate::host::Lines>> {
        self.st.host_lines.borrow().clone()
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

    /// A Table menu item; see [`TableCommand`].
    pub fn table(&self, cmd: TableCommand) {
        // A composition in progress is the document's before a table is
        // re-spelled around it, as before a key.
        self.st
            .composer
            .borrow_mut()
            .settle(&mut self.st.doc.borrow_mut());
        let action = apply_table(&mut self.st.doc.borrow_mut(), cmd);
        trace(|| format!("table {cmd:?} -> {action:?}"));
        if action == Action::Handled {
            self.refresh();
            self.ime_area();
        }
    }

    /// The window's events the editor reads, but for its keys: the input
    /// method, the surface's size, a click's release, and the theme. A host
    /// hands it every event, and the keys separately ([`crate::pressed`],
    /// [`Self::key`]), so that it decides which keys are the editor's.
    pub fn window_event(&self, event: &WindowEvent) {
        match event {
            WindowEvent::Ime(ime) => self.ime(ime),
            WindowEvent::SurfaceResized(size) => self.resize(size.width, size.height),
            // A finger: a tap, a scroll or a turn (`touch.rs`). Its release
            // tells the input method where the caret is, as a click's does.
            WindowEvent::PointerButton {
                state,
                button: ButtonSource::Touch { .. },
                position,
                ..
            } => {
                self.touch_button(state.is_pressed(), *position);
                if !state.is_pressed() {
                    self.ime_area();
                }
            }
            WindowEvent::PointerMoved {
                source: PointerSource::Touch { .. },
                position,
                ..
            } => self.touch_moved(*position),
            // A mouse or a pen, after a finger: the rows hear presses again.
            WindowEvent::PointerButton { state, .. } if state.is_pressed() => {
                self.st.touch_mode.set(false)
            }
            // A click moved the caret: on the button's release, which winit
            // reports after Blitz has handled the press, tell the input method
            // where it went and what is around it now.
            WindowEvent::PointerButton { state, .. } if !state.is_pressed() => self.ime_area(),
            // Blitz paints nothing while the window is covered, so a capture
            // of a covered window is its last frame — which looks exactly like
            // a hung editor. Said here so the trace tells them apart.
            WindowEvent::Occluded(o) => trace(|| format!("occluded {o}")),
            // Light or dark. Blitz turns the theme into the stylesheet's
            // `prefers-color-scheme` and restyles, but asks for no frame, so
            // the window would wait for the next event to show it.
            WindowEvent::ThemeChanged(theme) => {
                trace(|| format!("theme {theme:?}"));
                // Formulas and diagrams are inked for the appearance.
                self.reink();
                self.repaint();
                self.st.window.request_redraw();
            }
            _ => {}
        }
    }

    /// The input method: a composition's steps, its commit, or text around
    /// the caret to delete first (press-and-hold). See `ime.rs`.
    fn ime(&self, ime: &Ime) {
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
                Ime::DeleteSurrounding {
                    before_bytes,
                    after_bytes,
                } => c.delete_surrounding(&mut d, *before_bytes, *after_bytes),
                Ime::Disabled => c.settle(&mut d),
                _ => return,
            }
        }
        self.refresh();
        self.ime_area();
    }

    /// Allow the input method on the window, unless it is already allowed
    /// — on iOS, raising the keyboard (winit-uikit's `becomeFirstResponder`)
    /// — or the document is only to be read. Surrounding text needs the
    /// patched winit-appkit to be heard on macOS (see the Blitz fork's
    /// `patches/winit-appkit/PATCH.md`); winit asks for some with the
    /// capability, so an empty line to start.
    pub(crate) fn enable_ime(&self) {
        if self.st.window.ime_capabilities().is_some() || self.st.doc.borrow().read_only() {
            return;
        }
        let request = ImeEnableRequest::new(
            ImeCapabilities::new()
                .with_cursor_area()
                .with_surrounding_text(),
            ImeRequestData::default()
                .with_cursor_area(
                    LogicalPosition::new(0.0, 0.0).into(),
                    LogicalSize::new(2.0, LINE).into(),
                )
                .with_surrounding_text(
                    ImeSurroundingText::new(String::new(), 0, 0).expect("an empty excerpt"),
                ),
        );
        if let Some(request) = request {
            let _ = self
                .st
                .window
                .request_ime_update(ImeRequest::Enable(request));
        }
    }

    /// Disallow the input method on the window, which lowers an on-screen
    /// keyboard: the editor is leaving the page.
    pub(crate) fn disable_ime(&self) {
        if self.st.window.ime_capabilities().is_some() {
            let _ = self.st.window.request_ime_update(ImeRequest::Disable);
        }
    }

    /// Tell the input method where the caret is, so its candidate window and
    /// the accent menu open beside it, and what text surrounds it, so
    /// press-and-hold can see the letter it offers accents for.
    pub fn ime_area(&self) {
        // Not drawn, the host knows where the caret is, and the rows, which
        // are not kept, are not asked.
        if !self.st.drawn.get() {
            let Some((x, y, height)) = self.st.host.caret_area(self) else {
                return;
            };
            let mut data = ImeRequestData::default().with_cursor_area(
                LogicalPosition::new(x, y).into(),
                LogicalSize::new(2.0, height).into(),
            );
            let around = ime::surrounding(&self.st.doc.borrow(), &self.st.composer.borrow());
            self.st.host.input_area(&InputArea {
                x,
                y,
                height,
                around: None,
            });
            if let Some(text) = around.and_then(|(t, c, a)| ImeSurroundingText::new(t, c, a).ok()) {
                data = data.with_surrounding_text(text);
            }
            let _ = self.st.window.request_ime_update(ImeRequest::Update(data));
            return;
        }
        let Some((row, line, x)) = self.caret_at() else {
            return;
        };
        let lays = self.sig.lays.peek();
        let Some(l) = lays.get(row).and_then(|l| l.0.lines.get(line)) else {
            return;
        };
        let y = self.sig.layout.peek().top(row) - *self.sig.top.peek()
            + l.top as f64
            + self.sig.bar.peek().height()
            + self.sig.origin.peek().1;
        let mut data = ImeRequestData::default().with_cursor_area(
            LogicalPosition::new(self.left() + x as f64, y).into(),
            LogicalSize::new(2.0, l.height as f64).into(),
        );
        let around = ime::surrounding(&self.st.doc.borrow(), &self.st.composer.borrow());
        self.st.host.input_area(&InputArea {
            x: self.left() + x as f64,
            y,
            height: l.height as f64,
            around: self.shown_around(),
        });
        if let Some(text) = around.and_then(|(t, c, a)| ImeSurroundingText::new(t, c, a).ok()) {
            data = data.with_surrounding_text(text);
        }
        let _ = self.st.window.request_ime_update(ImeRequest::Update(data));
    }
}
