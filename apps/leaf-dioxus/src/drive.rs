//! A debug build's remote hand: `LEAF_DRIVE=<path>` names a FIFO whose
//! lines are pointer gestures, delivered to the front document window as
//! the winit events a real pointer makes, through the same handler. How a
//! session with no way to move the system's pointer — an agent's, a CI
//! job's — clicks in the page and opens its menus.
//!
//! ```text
//! click X Y          press and release the main button at (X, Y) CSS pixels
//! right X Y          the same with the secondary button
//! move X Y           move the pointer there
//! drag X Y X2 Y2     press at one point, move, release at the other
//! type TEXT          each character of TEXT as a key press
//! key NAME [MODS]    a named key (Enter, Escape, Tab, an arrow, Backspace) or
//!                    a character, with modifiers: cmd, shift, alt, ctrl
//! window TITLE       send what follows to the window of that title
//! ```
//!
//! A key goes the way a keyboard's does: to an open menu or dialog first,
//! then the editor. Menu bar items are pressed through the accessibility
//! API instead, which reaches the app's own menus.

use std::io::BufRead;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

use dioxus_native::winit::dpi::PhysicalPosition;
use dioxus_native::winit::event::{
    ButtonSource, ElementState, KeyEvent, MouseButton, PointerSource, WindowEvent,
};
use dioxus_native::winit::keyboard::{
    Key, KeyCode, KeyLocation, ModifiersState, NamedKey, PhysicalKey, SmolStr,
};

static LINES: Mutex<Option<Receiver<String>>> = Mutex::new(None);

/// The window gestures go to, by title, once a `window` line names one.
pub static TARGET: Mutex<Option<String>> = Mutex::new(None);

/// Start reading `LEAF_DRIVE`, if it is set; `wake` is called after each
/// line, to bring the event loop round to [`take`].
pub fn start(wake: impl Fn() + Send + 'static) {
    let Some(path) = std::env::var_os("LEAF_DRIVE") else {
        return;
    };
    let (tx, rx): (Sender<String>, Receiver<String>) = channel();
    *LINES.lock().unwrap() = Some(rx);
    std::thread::spawn(move || {
        loop {
            // A FIFO's reader sees its end each time a writer closes it.
            let Ok(f) = std::fs::File::open(&path) else {
                return;
            };
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
                wake();
            }
        }
    });
}

/// The events the lines read since the last call stand for, at `scale`.
pub fn take(scale: f64) -> Vec<WindowEvent> {
    let lines: Vec<String> = match LINES.lock().unwrap().as_ref() {
        Some(rx) => rx.try_iter().collect(),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for l in &lines {
        if let Some(title) = l.strip_prefix("window ") {
            *TARGET.lock().unwrap() = Some(title.trim().to_string());
            continue;
        }
        out.extend(events(l, scale));
    }
    out
}

fn events(line: &str, scale: f64) -> Vec<WindowEvent> {
    let w: Vec<&str> = line.split_whitespace().collect();
    let n = |i: usize| w.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * scale;
    let at = |x, y| PhysicalPosition::new(x, y);
    let moved = |p| WindowEvent::PointerMoved {
        device_id: None,
        position: p,
        primary: true,
        source: PointerSource::Mouse,
    };
    let button = |p, pressed: bool, b| WindowEvent::PointerButton {
        device_id: None,
        state: if pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        },
        position: p,
        primary: true,
        button: ButtonSource::Mouse(b),
        is_macos_activation_click: false,
    };
    match w.first().copied() {
        Some("move") => vec![moved(at(n(1), n(2)))],
        Some(c @ ("click" | "right")) => {
            let b = if c == "right" {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            let p = at(n(1), n(2));
            vec![moved(p), button(p, true, b), button(p, false, b)]
        }
        Some("drag") => {
            let (a, b) = (at(n(1), n(2)), at(n(3), n(4)));
            vec![
                moved(a),
                button(a, true, MouseButton::Left),
                moved(b),
                button(b, false, MouseButton::Left),
            ]
        }
        Some("type") => {
            let text = line
                .trim_start()
                .strip_prefix("type")
                .unwrap_or("")
                .trim_start();
            text.chars()
                .flat_map(|c| {
                    press(
                        Key::Character(SmolStr::new(c.to_string())),
                        Some(c.to_string()),
                        code_of(c),
                    )
                })
                .collect()
        }
        Some("key") => {
            let name = w.get(1).copied().unwrap_or("");
            let mut mods = ModifiersState::empty();
            for m in &w[2.min(w.len())..] {
                mods |= match *m {
                    "cmd" => ModifiersState::META,
                    "shift" => ModifiersState::SHIFT,
                    "alt" => ModifiersState::ALT,
                    "ctrl" => ModifiersState::CONTROL,
                    _ => ModifiersState::empty(),
                };
            }
            let (key, text, code) = match named(name) {
                Some((k, code)) => (Key::Named(k), None, code),
                None => {
                    let c = name.chars().next().unwrap_or(' ');
                    let text = mods.is_empty().then(|| name.to_string());
                    (Key::Character(SmolStr::new(name)), text, code_of(c))
                }
            };
            let mut v = vec![WindowEvent::ModifiersChanged(mods.into())];
            v.extend(press(key, text, code));
            v.push(WindowEvent::ModifiersChanged(
                ModifiersState::empty().into(),
            ));
            v
        }
        _ => {
            eprintln!("drive: {line:?}?");
            Vec::new()
        }
    }
}

/// A key's press and release.
fn press(key: Key, text: Option<String>, code: KeyCode) -> Vec<WindowEvent> {
    let event = |state| WindowEvent::KeyboardInput {
        device_id: None,
        event: KeyEvent {
            physical_key: PhysicalKey::Code(code),
            logical_key: key.clone(),
            text: text
                .clone()
                .filter(|_| state == ElementState::Pressed)
                .map(SmolStr::new),
            location: KeyLocation::Standard,
            state,
            repeat: false,
            text_with_all_modifiers: text.clone().map(SmolStr::new),
            key_without_modifiers: key.clone(),
        },
        is_synthetic: false,
    };
    vec![event(ElementState::Pressed), event(ElementState::Released)]
}

fn named(name: &str) -> Option<(NamedKey, KeyCode)> {
    Some(match name {
        "Enter" => (NamedKey::Enter, KeyCode::Enter),
        "Escape" => (NamedKey::Escape, KeyCode::Escape),
        "Backspace" => (NamedKey::Backspace, KeyCode::Backspace),
        "Tab" => (NamedKey::Tab, KeyCode::Tab),
        "ArrowUp" => (NamedKey::ArrowUp, KeyCode::ArrowUp),
        "ArrowDown" => (NamedKey::ArrowDown, KeyCode::ArrowDown),
        "ArrowLeft" => (NamedKey::ArrowLeft, KeyCode::ArrowLeft),
        "ArrowRight" => (NamedKey::ArrowRight, KeyCode::ArrowRight),
        _ => return None,
    })
}

/// The physical key a US layout types `c` with.
fn code_of(c: char) -> KeyCode {
    match c.to_ascii_lowercase() {
        'a'..='z' => {
            const KEYS: [KeyCode; 26] = [
                KeyCode::KeyA,
                KeyCode::KeyB,
                KeyCode::KeyC,
                KeyCode::KeyD,
                KeyCode::KeyE,
                KeyCode::KeyF,
                KeyCode::KeyG,
                KeyCode::KeyH,
                KeyCode::KeyI,
                KeyCode::KeyJ,
                KeyCode::KeyK,
                KeyCode::KeyL,
                KeyCode::KeyM,
                KeyCode::KeyN,
                KeyCode::KeyO,
                KeyCode::KeyP,
                KeyCode::KeyQ,
                KeyCode::KeyR,
                KeyCode::KeyS,
                KeyCode::KeyT,
                KeyCode::KeyU,
                KeyCode::KeyV,
                KeyCode::KeyW,
                KeyCode::KeyX,
                KeyCode::KeyY,
                KeyCode::KeyZ,
            ];
            KEYS[(c.to_ascii_lowercase() as u8 - b'a') as usize]
        }
        ' ' => KeyCode::Space,
        '.' => KeyCode::Period,
        ',' => KeyCode::Comma,
        '/' => KeyCode::Slash,
        ':' => KeyCode::Semicolon,
        '-' => KeyCode::Minus,
        _ => KeyCode::Backquote,
    }
}
