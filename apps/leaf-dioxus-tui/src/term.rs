//! The terminal side: raw mode and the alternate screen, a grid written out
//! as the cells that changed, and the terminal's keys and mouse turned into
//! what the editor and Blitz take.

use std::io::{Stdout, Write, stdout};
use std::str::FromStr;

use anyhow::Result;
use blitz_traits::events::{
    BlitzPointerEvent, BlitzPointerId, MouseEventButton, MouseEventButtons, PointerCoords, UiEvent,
};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyCode,
    KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::style::{
    Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::{cursor, execute, queue, terminal};
use dioxus::html::{Code, Key, Modifiers, NamedKey};
use dioxus_native::DioxusDocument;

use crate::paint::{Cell, Grid, Metrics};

/// Dark unless `LEAF_THEME=light` says otherwise; leaf-tui's OSC 11 query
/// is the real answer, and is for later.
pub fn dark() -> bool {
    std::env::var("LEAF_THEME").map_or(true, |t| t != "light")
}

pub struct Terminal {
    out: Stdout,
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
            terminal::Clear(terminal::ClearType::All)
        )?;
        Ok(Self { out })
    }

    pub fn leave(&mut self) -> Result<()> {
        execute!(
            self.out,
            DisableBracketedPaste,
            DisableMouseCapture,
            cursor::Show,
            terminal::LeaveAlternateScreen
        )?;
        terminal::disable_raw_mode()?;
        Ok(())
    }

    pub fn draw(&mut self, g: &Grid) -> Result<()> {
        queue!(self.out, cursor::Hide)?;
        for r in 0..g.rows {
            queue!(self.out, cursor::MoveTo(0, r))?;
            for c in 0..g.cols {
                self.cell(g.get(c, r))?;
            }
        }
        self.finish(g)
    }

    pub fn draw_diff(&mut self, old: &Grid, new: &Grid) -> Result<()> {
        if old.cols != new.cols || old.rows != new.rows {
            return self.draw(new);
        }
        queue!(self.out, cursor::Hide)?;
        for r in 0..new.rows {
            let mut at = None;
            for c in 0..new.cols {
                let cell = new.get(c, r);
                if cell == old.get(c, r) {
                    at = None;
                    continue;
                }
                if at != Some(c) {
                    queue!(self.out, cursor::MoveTo(c, r))?;
                }
                self.cell(cell)?;
                at = Some(c + 1);
            }
        }
        self.finish(new)
    }

    fn cell(&mut self, cell: &Cell) -> Result<()> {
        // The second half of a wide character: its first half drew it.
        if cell.text.is_empty() {
            return Ok(());
        }
        let rgb = |(r, g, b): (u8, u8, u8)| Color::Rgb { r, g, b };
        queue!(
            self.out,
            SetAttribute(Attribute::Reset),
            SetForegroundColor(rgb(cell.fg)),
            SetBackgroundColor(rgb(cell.bg)),
        )?;
        if cell.bold {
            queue!(self.out, SetAttribute(Attribute::Bold))?;
        }
        if cell.italic {
            queue!(self.out, SetAttribute(Attribute::Italic))?;
        }
        if cell.underline {
            queue!(self.out, SetAttribute(Attribute::Underlined))?;
        }
        queue!(self.out, Print(&cell.text))?;
        Ok(())
    }

    fn finish(&mut self, g: &Grid) -> Result<()> {
        queue!(self.out, SetAttribute(Attribute::Reset))?;
        if let Some((c, r)) = g.cursor {
            queue!(
                self.out,
                cursor::MoveTo(c, r),
                cursor::SetCursorStyle::SteadyBar,
                cursor::Show
            )?;
        }
        self.out.flush()?;
        Ok(())
    }
}

pub fn is_quit(k: &KeyEvent) -> bool {
    k.kind != KeyEventKind::Release
        && k.modifiers.contains(KeyModifiers::CONTROL)
        && k.code == KeyCode::Char('q')
}

/// The editor's primary modifier: ⌘ on macOS, where a terminal hands over ⌃
/// instead, and ⌃ everywhere else.
const PRIMARY: Modifiers = if cfg!(target_os = "macos") {
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

/// A left-button press, drag or release, as Blitz's pointer events in the
/// cell it fell in: in its left part, so a click on a character puts the
/// caret before it, as a terminal's block cursor reads.
pub fn pointer(doc: &mut DioxusDocument, m: Metrics, me: &MouseEvent) {
    let x = (me.column as f32 + 0.3) * m.cw;
    let y = (me.row as f32 + 0.5) * m.ch;
    let scroll = doc.inner.borrow().viewport_scroll();
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::SHIFT, me.modifiers.contains(KeyModifiers::SHIFT));
    let held = !matches!(me.kind, MouseEventKind::Up(_));
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
        button: MouseEventButton::Main,
        buttons: if held {
            MouseEventButtons::Primary
        } else {
            MouseEventButtons::None
        },
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
