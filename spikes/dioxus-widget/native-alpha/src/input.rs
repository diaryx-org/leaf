//! A key to a [`Doc`] edit — the job `leaf-ratatui`'s `input.rs` does for
//! crossterm and `leaf-gpui` does for gpui, here for a DOM keyboard event.
//!
//! It is deliberately a *function of the key*, not of Dioxus: the component
//! hands it `Key` and `Modifiers` and nothing else, so the mapping is testable
//! without a virtual DOM, a browser, or an event. That is also what makes the
//! answer to "can Dioxus express leaf's input?" legible — everything hard
//! about it is in the ten lines of [`Action`] below, and what Dioxus has to
//! supply is one `onkeydown`.
//!
//! A copy of the parent's `src/input.rs`, minus its tests: `Key` and
//! `Modifiers` are Dioxus types, and this crate's Dioxus (0.8-alpha) is not
//! the parent's (0.7), so the two cannot share it. The tests stay in the
//! parent, where they cover the same mapping.

use dioxus::html::{Key, Modifiers};
use leaf_core::Doc;

/// What a key did, for the host to act on what it does not own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// The document changed, or the caret moved: repaint.
    Handled,
    /// Not ours — let the browser have it.
    Ignored,
    /// The host owns this one: the clipboard, which a DOM widget cannot read
    /// from a key event at all (it needs the `paste` event's `DataTransfer`),
    /// and saving.
    Host(Host),
}

/// The gestures a widget cannot complete by itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Host {
    Copy,
    Cut,
    Paste,
    Save,
}

/// Apply one key press to the document.
///
/// `shift` extends the selection on every motion, which is why each motion
/// passes it rather than branching on it: core's caret API takes `extend` for
/// exactly this.
pub fn apply_key(doc: &mut Doc, key: &Key, mods: Modifiers) -> Action {
    let shift = mods.contains(Modifiers::SHIFT);
    // The command modifier: Meta on Apple, Control elsewhere. A widget cannot
    // tell which platform it is on from a key event, so it honours both — the
    // same thing leaf-web does.
    let cmd = mods.contains(Modifiers::META) || mods.contains(Modifiers::CONTROL);

    if cmd && let Key::Character(c) = key {
        return match c.as_str() {
            "c" => Action::Host(Host::Copy),
            "x" => Action::Host(Host::Cut),
            "v" => Action::Host(Host::Paste),
            "s" => Action::Host(Host::Save),
            "a" => {
                doc.select_all();
                Action::Handled
            }
            "z" if shift => {
                doc.redo();
                Action::Handled
            }
            "z" => {
                doc.undo();
                Action::Handled
            }
            _ => Action::Ignored,
        };
    }

    match key {
        // A printable key. `Key::Character` is already the composed result of
        // the layout and the modifiers, so there is no keycode arithmetic here
        // — but see README.md on what it is *not*: an IME's composition never
        // arrives this way.
        Key::Character(text) if !cmd => {
            doc.insert(text);
            Action::Handled
        }
        Key::Enter => {
            doc.newline();
            Action::Handled
        }
        Key::Backspace => {
            doc.backspace();
            Action::Handled
        }
        Key::Delete => {
            doc.delete_forward();
            Action::Handled
        }
        Key::Tab => {
            if shift {
                doc.outdent();
            } else {
                doc.indent();
            }
            Action::Handled
        }
        Key::ArrowLeft => {
            if mods.contains(Modifiers::ALT) {
                doc.move_word_left(shift);
            } else {
                doc.move_left(shift);
            }
            Action::Handled
        }
        Key::ArrowRight => {
            if mods.contains(Modifiers::ALT) {
                doc.move_word_right(shift);
            } else {
                doc.move_right(shift);
            }
            Action::Handled
        }
        Key::ArrowUp => {
            doc.move_up(shift);
            Action::Handled
        }
        Key::ArrowDown => {
            doc.move_down(shift);
            Action::Handled
        }
        Key::Home => {
            if cmd {
                doc.move_doc_start(shift);
            } else {
                doc.move_home(shift);
            }
            Action::Handled
        }
        Key::End => {
            if cmd {
                doc.move_doc_end(shift);
            } else {
                doc.move_end(shift);
            }
            Action::Handled
        }
        _ => Action::Ignored,
    }
}

