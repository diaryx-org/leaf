//! What leaf adds to blitz-tui's terminal: the theme leaf-tui's way, the
//! editor's keys, the pictures leaf-raster reads, and clicks shifted onto
//! the editor's text.

use anyhow::Result;
use blitz_tui::Metrics;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use dioxus::html::{Code, Key, Modifiers};
use dioxus_native::DioxusDocument;

thread_local! {
    static DARK: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether the terminal is dark: `LEAF_THEME=light|dark` if set, else the
/// answer [`ask_theme`] got, else dark — leaf-tui's order.
pub fn dark() -> bool {
    match std::env::var("LEAF_THEME").as_deref() {
        Ok("light") => false,
        Ok("dark") => true,
        _ => DARK.get().unwrap_or(true),
    }
}

/// Ask the terminal whether it is light or dark, before anything is drawn
/// to its colours.
pub fn ask_theme() {
    if let Some(dark) = blitz_tui::ask_dark() {
        DARK.set(Some(dark));
    }
}

/// blitz-tui's terminal, reading pictures as leaf-tui does.
pub fn enter() -> Result<blitz_tui::Terminal> {
    let mut term = blitz_tui::Terminal::enter()?;
    term.set_decoder(Box::new(crate::pictures::decode));
    crate::trace(|| format!("graphics: {:?}", term.pictures().protocol()));
    Ok(term)
}

/// The editor's primary modifier: ⌘ on macOS, where a terminal hands over ⌃
/// instead, and ⌃ everywhere else.
pub const PRIMARY: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

/// A terminal key as the editor's `(Key, Code, Modifiers)`, the shape
/// `leaf_dioxus::pressed` makes of winit's: ⌃ as [`PRIMARY`].
pub fn key(k: &KeyEvent) -> Option<(Key, Code, Modifiers)> {
    if k.kind == KeyEventKind::Release {
        return None;
    }
    let e = blitz_tui::input::key(k)?;
    let mut mods = e.modifiers;
    if mods.contains(Modifiers::CONTROL) {
        mods.remove(Modifiers::CONTROL);
        mods.insert(PRIMARY);
    }
    // ⌃K is the editor's own ⌃ chord on Apple (delete to the line's end),
    // as in leaf-tui; ⌘K has nothing to lose to it.
    if k.code == KeyCode::Char('k') && k.modifiers == KeyModifiers::CONTROL {
        mods = Modifiers::CONTROL;
    }
    // ⌃⇧z arrives as `Z` with SHIFT already folded in.
    let key = match e.key {
        Key::Character(s) if mods.intersects(PRIMARY) => Key::Character(s.to_lowercase()),
        k => k,
    };
    Some((key, e.code, mods))
}

/// A typed character, with the physical key a US layout gives it.
pub fn char_key(c: char) -> (Key, Code, Modifiers) {
    let e = blitz_tui::input::char_key(c);
    (e.key, e.code, e.modifiers)
}

/// A press, a drag, a release or a move, to Blitz. The editor's text starts
/// `PAD` in, which the painter rounds to a whole cell; a click is shifted
/// the same way, so it lands on the character drawn under it.
pub fn pointer(doc: &mut DioxusDocument, m: Metrics, me: &MouseEvent) {
    let shift = m.shift(leaf_dioxus::PAD as f32);
    let ui = blitz_tui::input::mouse(&doc.inner.borrow(), me, m, shift);
    if let Some(ui) = ui {
        blitz_dom::Document::handle_ui_event(doc, ui);
    }
}
