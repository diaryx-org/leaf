//! leaf — a caret-based rich-text TUI editor for documents, built on twig.
//!
//! Sibling to bough: same twig backend, opposite interaction model. bough moves
//! a selection through the AST and edits the tree; leaf gives you a text caret,
//! mouse, and a formatting toolbar, and turns each keystroke into an
//! offset-addressed twig edit that reparses live. You type into a document that
//! stays a valid AST the whole time.

mod style;
mod ui;

use std::io::stdout;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use ratatui::crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
};
use leaf_core::{BlockKind, Doc, InlineKind};

fn main() -> Result<()> {
    let arg = std::env::args_os()
        .nth(1)
        .ok_or_else(|| anyhow!("usage: leaf <file.md|file.dj|file.html|file.xml>"))?;
    let mut doc = Doc::open(PathBuf::from(arg))?;

    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let result = run(&mut terminal, &mut doc);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, doc: &mut Doc) -> Result<()> {
    loop {
        let breadcrumb = doc.breadcrumb();
        terminal.draw(|f| ui::render(f, doc, &breadcrumb))?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(doc, key) == Flow::Quit {
                    return Ok(());
                }
            }
            Event::Mouse(m) => handle_mouse(doc, m),
            _ => {}
        }
    }
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

fn handle_key(doc: &mut Doc, key: KeyEvent) -> Flow {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if ctrl {
        match key.code {
            KeyCode::Char('q') => return Flow::Quit,
            KeyCode::Char('s') => doc.save(),
            _ => {}
        }
        return Flow::Continue;
    }

    if alt {
        // The formatting toolbar. Inline marks act on the selection; heading /
        // body conversion acts on the block at the caret.
        match key.code {
            KeyCode::Char('w') => doc.toggle_view(),
            KeyCode::Char('b') => doc.toggle(InlineKind::Strong),
            KeyCode::Char('i') => doc.toggle(InlineKind::Emph),
            KeyCode::Char('c') => doc.toggle(InlineKind::Verbatim),
            KeyCode::Char('m') => doc.toggle(InlineKind::Mark),
            KeyCode::Char('0') => doc.set_block(BlockKind::Paragraph),
            KeyCode::Char(d @ '1'..='6') => {
                doc.set_block(BlockKind::Heading(d.to_digit(10).unwrap()))
            }
            _ => {}
        }
        return Flow::Continue;
    }

    match key.code {
        KeyCode::Char(c) => doc.insert(&c.to_string()),
        KeyCode::Enter => doc.insert("\n"),
        KeyCode::Tab => doc.insert("    "),
        KeyCode::Backspace => doc.backspace(),
        KeyCode::Delete => doc.delete_forward(),
        KeyCode::Left => doc.move_left(shift),
        KeyCode::Right => doc.move_right(shift),
        KeyCode::Up => doc.move_up(shift),
        KeyCode::Down => doc.move_down(shift),
        KeyCode::Home => doc.move_home(shift),
        KeyCode::End => doc.move_end(shift),
        _ => {}
    }
    Flow::Continue
}

fn handle_mouse(doc: &mut Doc, m: MouseEvent) {
    let (bx, by) = doc.body_origin;
    let within = m.row >= by
        && (m.row as usize) < by as usize + doc.body_height as usize
        && m.column >= bx;

    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = (m.column - bx) as usize;
            doc.click(row, col, false);
        }
        MouseEventKind::Drag(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = (m.column - bx) as usize;
            doc.click(row, col, true); // extend the selection
        }
        MouseEventKind::ScrollDown => doc.scroll = doc.scroll.saturating_add(1),
        MouseEventKind::ScrollUp => doc.scroll = doc.scroll.saturating_sub(1),
        _ => {}
    }
}
