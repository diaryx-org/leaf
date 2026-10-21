//! The terminal side: raw mode and the alternate screen, a grid written out
//! as the cells that changed, and the terminal's keys and mouse turned into
//! what the editor and Blitz take.

use std::io::{Stdout, stdout};
use std::str::FromStr;

use anyhow::Result;
use blitz_traits::events::{
    BlitzPointerEvent, BlitzPointerId, MouseEventButton, MouseEventButtons, PointerCoords, UiEvent,
};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyCode,
    KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::{cursor, execute, terminal};
use dioxus::html::{Code, Key, Modifiers, NamedKey};
use dioxus_native::DioxusDocument;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier};

use crate::paint::{Grid, Metrics};
use crate::pictures::Pictures;

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

/// Ask the terminal whether it is light or dark (an `OSC 11` query, with a
/// timeout, which terminal-colorsaurus answers from the controlling tty
/// without stealing a keystroke), before anything is drawn to its colours.
pub fn ask_theme() {
    if let Ok(palette) = terminal_colorsaurus::color_palette(Default::default()) {
        let dark = palette.theme_mode() == terminal_colorsaurus::ThemeMode::Dark;
        DARK.set(Some(dark));
    }
}

pub struct Terminal {
    term: ratatui::Terminal<CrosstermBackend<Stdout>>,
    pictures: Pictures,
}

impl Terminal {
    pub fn enter() -> Result<Self> {
        let mut out = stdout();
        terminal::enable_raw_mode()?;
        execute!(
            out,
            terminal::EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            cursor::SetCursorStyle::SteadyBar,
        )?;
        let mut pictures = Pictures::new();
        pictures.query();
        crate::trace(|| format!("graphics: {:?}", pictures.protocol()));
        let mut term = ratatui::Terminal::new(CrosstermBackend::new(out))?;
        term.clear()?;
        Ok(Self { term, pictures })
    }

    pub fn leave(&mut self) -> Result<()> {
        execute!(
            self.term.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            cursor::SetCursorStyle::DefaultUserShape,
            cursor::Show,
            terminal::LeaveAlternateScreen
        )?;
        terminal::disable_raw_mode()?;
        Ok(())
    }

    /// Forget what is on screen: the next draw writes every cell.
    pub fn clear(&mut self) -> Result<()> {
        self.term.clear()?;
        Ok(())
    }

    /// Draw `g`: its cells, then its pictures over them. ratatui writes only
    /// the cells that changed since the last frame.
    pub fn draw(&mut self, g: &Grid) -> Result<()> {
        let pictures = &mut self.pictures;
        self.term.draw(|f| {
            let area = f.area();
            let buf = f.buffer_mut();
            for r in 0..g.rows.min(area.height) {
                for c in 0..g.cols.min(area.width) {
                    let cell = g.get(c, r);
                    let out = &mut buf[(c, r)];
                    // The second half of a wide character: its first half
                    // drew it, and ratatui skips this cell.
                    out.set_symbol(if cell.text.is_empty() {
                        " "
                    } else {
                        &cell.text
                    });
                    out.set_fg(rgb(cell.fg));
                    out.set_bg(rgb(cell.bg));
                    let mut m = Modifier::empty();
                    m.set(Modifier::BOLD, cell.bold);
                    m.set(Modifier::ITALIC, cell.italic);
                    m.set(Modifier::UNDERLINED, cell.underline);
                    out.modifier = m;
                }
            }
            pictures.paint(f, g);
            if let Some((c, r)) = g.cursor {
                f.set_cursor_position((c, r));
            }
        })?;
        Ok(())
    }
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

/// The editor's primary modifier: ⌘ on macOS, where a terminal hands over ⌃
/// instead, and ⌃ everywhere else.
pub const PRIMARY: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

/// A terminal key as the editor's `(Key, Code, Modifiers)`, the shape
/// `leaf_dioxus::pressed` makes of winit's.
pub fn key(k: &KeyEvent) -> Option<(Key, Code, Modifiers)> {
    if k.kind == KeyEventKind::Release {
        return None;
    }
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::SHIFT, k.modifiers.contains(KeyModifiers::SHIFT));
    mods.set(Modifiers::ALT, k.modifiers.contains(KeyModifiers::ALT));
    mods.set(PRIMARY, k.modifiers.contains(KeyModifiers::CONTROL));
    // ⌃K is the editor's own ⌃ chord on Apple (delete to the line's end),
    // as in leaf-tui; ⌘K has nothing to lose to it.
    if k.code == KeyCode::Char('k') && k.modifiers == KeyModifiers::CONTROL {
        mods = Modifiers::CONTROL;
    }
    if k.code == KeyCode::BackTab {
        mods.insert(Modifiers::SHIFT);
    }
    let named = |n| Some((Key::Named(n), Code::Unidentified, mods));
    match k.code {
        KeyCode::Char(c) => {
            let (key, code, _) = char_key(c);
            // ⌃⇧z arrives as `Z` with SHIFT already folded in.
            let key = if mods.intersects(PRIMARY) {
                match key {
                    Key::Character(s) => Key::Character(s.to_lowercase()),
                    k => k,
                }
            } else {
                key
            };
            Some((key, code, mods))
        }
        KeyCode::Enter => named(NamedKey::Enter),
        KeyCode::Backspace => named(NamedKey::Backspace),
        KeyCode::Delete => named(NamedKey::Delete),
        KeyCode::Tab => named(NamedKey::Tab),
        KeyCode::BackTab => named(NamedKey::Tab),
        KeyCode::Esc => named(NamedKey::Escape),
        KeyCode::Left => named(NamedKey::ArrowLeft),
        KeyCode::Right => named(NamedKey::ArrowRight),
        KeyCode::Up => named(NamedKey::ArrowUp),
        KeyCode::Down => named(NamedKey::ArrowDown),
        KeyCode::Home => named(NamedKey::Home),
        KeyCode::End => named(NamedKey::End),
        KeyCode::PageUp => named(NamedKey::PageUp),
        KeyCode::PageDown => named(NamedKey::PageDown),
        KeyCode::F(n) => Key::from_str(&format!("F{n}"))
            .ok()
            .map(|k| (k, Code::Unidentified, mods)),
        _ => None,
    }
}

/// A typed character, with the physical key a US layout gives it.
pub fn char_key(c: char) -> (Key, Code, Modifiers) {
    if c == '\n' || c == '\r' {
        return (Key::Named(NamedKey::Enter), Code::Enter, Modifiers::empty());
    }
    let code = if c.is_ascii_alphabetic() {
        Code::from_str(&format!("Key{}", c.to_ascii_uppercase())).unwrap_or(Code::Unidentified)
    } else if c.is_ascii_digit() {
        Code::from_str(&format!("Digit{c}")).unwrap_or(Code::Unidentified)
    } else if c == ' ' {
        Code::Space
    } else {
        Code::Unidentified
    };
    (Key::Character(c.to_string()), code, Modifiers::empty())
}

/// A press, a drag, a release or a move, as Blitz's pointer events in the
/// cell it fell in: in its left part, so a click on a character puts the
/// caret before it, as a terminal's block cursor reads. A move with no
/// button down is the hover a menu highlights its items by.
pub fn pointer(doc: &mut DioxusDocument, m: Metrics, me: &MouseEvent) {
    // The editor's text starts `PAD` in, which the painter rounds to a
    // whole cell; a click is shifted the same way, so it lands on the
    // character drawn under it.
    let pad = leaf_dioxus::PAD as f32;
    let shift = pad - (pad / m.cw).round() * m.cw;
    let x = (me.column as f32 + 0.3) * m.cw + shift;
    let y = (me.row as f32 + 0.5) * m.ch;
    let scroll = doc.inner.borrow().viewport_scroll();
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::SHIFT, me.modifiers.contains(KeyModifiers::SHIFT));
    let button = match me.kind {
        MouseEventKind::Down(b) | MouseEventKind::Up(b) | MouseEventKind::Drag(b) => b,
        _ => MouseButton::Left,
    };
    let (button, held) = match button {
        MouseButton::Left => (MouseEventButton::Main, MouseEventButtons::Primary),
        MouseButton::Right => (MouseEventButton::Secondary, MouseEventButtons::Secondary),
        MouseButton::Middle => (MouseEventButton::Auxiliary, MouseEventButtons::Auxiliary),
    };
    let buttons = match me.kind {
        MouseEventKind::Down(_) | MouseEventKind::Drag(_) => held,
        _ => MouseEventButtons::None,
    };
    let event = BlitzPointerEvent {
        id: BlitzPointerId::Mouse,
        is_primary: true,
        coords: PointerCoords {
            page_x: x + scroll.x as f32,
            page_y: y + scroll.y as f32,
            screen_x: x,
            screen_y: y,
            client_x: x,
            client_y: y,
        },
        button,
        buttons,
        mods,
        details: Default::default(),
        element: Default::default(),
        active_pointers: Default::default(),
    };
    let ui = match me.kind {
        MouseEventKind::Down(_) => UiEvent::PointerDown(event),
        MouseEventKind::Up(_) => UiEvent::PointerUp(event),
        _ => UiEvent::PointerMove(event),
    };
    blitz_dom::Document::handle_ui_event(doc, ui);
}
