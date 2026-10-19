//! Keys from winit, not from Blitz.
//!
//! Blitz dispatches `keydown` to the focused node — or, with nothing focused,
//! to the root element — and its default actions clear focus: a click on
//! anything but a form control does, and so, on a double-click, does
//! something `preventDefault` on `mousedown` and `click` does not reach. So
//! `.leaf`'s `onkeydown` went deaf at random, and the editor cannot take
//! focus back: `MountedData::set_focus` from a handler is the same "RefCell
//! already borrowed" panic as scrolling from one.
//!
//! An editor that owns the window has no need of DOM focus to know where its
//! keys go. `use_window_event` sees every winit event before Blitz does, so
//! the keyboard is read there, converted to the `keyboard-types` values the
//! rest of the editor already speaks, and Blitz's copy of it is ignored.

use std::str::FromStr;

use dioxus::html::{Code, Key, Modifiers, NamedKey};
use dioxus_native::winit::event::KeyEvent;
use dioxus_native::winit::keyboard::{self, ModifiersState, PhysicalKey};

/// A pressed key, as [`crate::input::apply_key`] takes it; `None` for a
/// release, which the editor has no use for.
pub fn pressed(event: &KeyEvent, mods: ModifiersState) -> Option<(Key, Code, Modifiers)> {
    if !event.state.is_pressed() {
        return None;
    }
    Some((
        key(&event.logical_key),
        code(&event.physical_key),
        modifiers(mods),
    ))
}

/// winit's named keys carry the W3C names `keyboard-types` parses, so the
/// `Debug` spelling is the conversion; a character is itself.
///
/// But for a line break: the iOS keyboard's Return is not a named key —
/// winit-uikit hears it as `insertText("\n")` and reports a character, which
/// the editor would insert as a soft break inside the paragraph. It is Enter.
fn key(k: &keyboard::Key) -> Key {
    match k {
        keyboard::Key::Character(s) if s.as_str() == "\n" || s.as_str() == "\r" => {
            Key::Named(NamedKey::Enter)
        }
        keyboard::Key::Character(s) => Key::Character(s.to_string()),
        keyboard::Key::Named(n) => {
            Key::from_str(&format!("{n:?}")).unwrap_or(Key::Named(NamedKey::Unidentified))
        }
        _ => Key::Named(NamedKey::Unidentified),
    }
}

/// The same for physical keys: `KeyB`, `Digit8`, `BracketRight`.
fn code(k: &PhysicalKey) -> Code {
    match k {
        PhysicalKey::Code(c) => Code::from_str(&format!("{c:?}")).unwrap_or(Code::Unidentified),
        PhysicalKey::Unidentified(_) => Code::Unidentified,
    }
}

/// winit's `meta` is ⌘, which `keyboard-types` — and so Blitz, and so the
/// key mapping — calls `META`.
fn modifiers(m: ModifiersState) -> Modifiers {
    let mut out = Modifiers::empty();
    out.set(Modifiers::SHIFT, m.shift_key());
    out.set(Modifiers::CONTROL, m.control_key());
    out.set(Modifiers::ALT, m.alt_key());
    out.set(Modifiers::META, m.meta_key());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus_native::winit::keyboard::{KeyCode, NamedKey as WinitNamed};

    #[test]
    fn named_keys_and_codes_convert_by_name() {
        assert_eq!(
            key(&keyboard::Key::Named(WinitNamed::ArrowDown)),
            Key::Named(NamedKey::ArrowDown)
        );
        assert_eq!(
            key(&keyboard::Key::Named(WinitNamed::PageUp)),
            Key::Named(NamedKey::PageUp)
        );
        assert_eq!(
            key(&keyboard::Key::Named(WinitNamed::Enter)),
            Key::Named(NamedKey::Enter)
        );
        assert_eq!(code(&PhysicalKey::Code(KeyCode::KeyB)), Code::KeyB);
        assert_eq!(code(&PhysicalKey::Code(KeyCode::Digit8)), Code::Digit8);
        assert_eq!(
            code(&PhysicalKey::Code(KeyCode::BracketRight)),
            Code::BracketRight
        );
    }

    #[test]
    fn a_typed_line_break_is_enter() {
        assert_eq!(
            key(&keyboard::Key::Character("\n".into())),
            Key::Named(NamedKey::Enter)
        );
        assert_eq!(
            key(&keyboard::Key::Character("\r".into())),
            Key::Named(NamedKey::Enter)
        );
        assert_eq!(
            key(&keyboard::Key::Character("a".into())),
            Key::Character("a".into())
        );
    }

    #[test]
    fn meta_is_super() {
        assert_eq!(modifiers(ModifiersState::META), Modifiers::META);
    }
}
