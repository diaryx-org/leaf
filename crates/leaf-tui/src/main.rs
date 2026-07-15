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
use std::time::{Duration, Instant};

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

/// UI-only state that doesn't belong on `Doc`: the quit-confirmation prompt
/// and mouse click-counting for double/triple-click. Doc stays the frontend-
/// neutral model; this is the crossterm-facing bookkeeping around it.
#[derive(Default)]
struct App {
    /// Set by Ctrl+Q on a dirty document; while true the footer shows the
    /// "quit without saving?" prompt and normal key handling is suspended.
    confirm_quit: bool,
    /// Timing and screen cell of the last left mouse-down, for detecting
    /// double/triple clicks.
    last_click: Option<ClickState>,
}

struct ClickState {
    at: Instant,
    row: u16,
    col: u16,
    /// 1 = single, 2 = double, 3 = triple; cycles back to 1 after that.
    count: u8,
}

/// Clicks within this long, on the same cell, extend the click count.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

fn run(terminal: &mut ratatui::DefaultTerminal, doc: &mut Doc) -> Result<()> {
    let mut app = App::default();
    loop {
        let breadcrumb = doc.breadcrumb();
        terminal.draw(|f| ui::render(f, doc, &breadcrumb, app.confirm_quit))?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(doc, key, &mut app) == Flow::Quit {
                    return Ok(());
                }
            }
            Event::Mouse(m) => handle_mouse(doc, m, &mut app),
            _ => {}
        }
    }
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

fn handle_key(doc: &mut Doc, key: KeyEvent, app: &mut App) -> Flow {
    // The quit-confirmation prompt takes over the keyboard until answered.
    if app.confirm_quit {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => return Flow::Quit,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.confirm_quit = false,
            _ => {} // anything else: leave the prompt up
        }
        return Flow::Continue;
    }

    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if ctrl {
        match key.code {
            KeyCode::Char('q') => {
                if doc.dirty {
                    app.confirm_quit = true;
                } else {
                    return Flow::Quit;
                }
            }
            KeyCode::Char('s') => doc.save(),
            KeyCode::Char('a') => doc.select_all(),
            KeyCode::Char('c') => clipboard_copy(doc),
            KeyCode::Char('x') => clipboard_cut(doc),
            KeyCode::Char('v') => clipboard_paste(doc),
            // ^Z undo, ^⇧Z or ^Y redo.
            KeyCode::Char('z') | KeyCode::Char('Z') if shift => doc.redo(),
            KeyCode::Char('z') | KeyCode::Char('Z') => doc.undo(),
            KeyCode::Char('y') | KeyCode::Char('Y') => doc.redo(),
            // ^Home / ^End jump to the document's start / end.
            KeyCode::Home => doc.move_doc_start(shift),
            KeyCode::End => doc.move_doc_end(shift),
            _ => {}
        }
        return Flow::Continue;
    }

    if alt {
        // The formatting toolbar. Inline marks act on the selection; heading /
        // body conversion acts on the block at the caret. Word motion/delete
        // share this modifier with ⌥w/b/i/c/m/0-6 since crossterm reports
        // Alt+Left/Right/Backspace/Delete as ordinary key codes plus ALT.
        match key.code {
            KeyCode::Left => doc.move_word_left(shift),
            KeyCode::Right => doc.move_word_right(shift),
            KeyCode::Backspace => doc.delete_word_back(),
            KeyCode::Delete => doc.delete_word_forward(),
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
        KeyCode::Enter => doc.newline(),
        // In a table, Tab walks the cells (Shift+Tab back); everywhere else it
        // indents as it always has.
        KeyCode::Tab if doc.cell_hop(true) => {}
        KeyCode::BackTab if doc.cell_hop(false) => {}
        KeyCode::Tab => doc.insert("    "),
        KeyCode::Backspace => doc.backspace(),
        KeyCode::Delete => doc.delete_forward(),
        KeyCode::Left => doc.move_left(shift),
        KeyCode::Right => doc.move_right(shift),
        KeyCode::Up => doc.move_up(shift),
        KeyCode::Down => doc.move_down(shift),
        KeyCode::Home => doc.move_home(shift),
        KeyCode::End => doc.move_end(shift),
        // Page motion: one bodyful of rows, one row kept for overlap.
        KeyCode::PageUp => {
            for _ in 0..page_rows(doc) {
                doc.move_up(shift);
            }
        }
        KeyCode::PageDown => {
            for _ in 0..page_rows(doc) {
                doc.move_down(shift);
            }
        }
        _ => {}
    }
    Flow::Continue
}

/// The page step: the body's visible rows minus one for overlap (at least one).
fn page_rows(doc: &Doc) -> usize {
    (doc.body_height as usize).saturating_sub(1).max(1)
}

fn handle_mouse(doc: &mut Doc, m: MouseEvent, app: &mut App) {
    let (bx, by) = doc.body_origin;
    let within = m.row >= by
        && (m.row as usize) < by as usize + doc.body_height as usize
        && m.column >= bx;

    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = (m.column - bx) as usize;
            let count = click_count(app, m.row, m.column);

            // Single click places the caret; double selects the word under
            // it; triple selects the whole line. All three start from the
            // same `click` hit-test so the row/col → offset mapping (source
            // bytes vs. the WYSIWYG glyph grid) only lives in one place.
            doc.click(row, col, false);
            match count {
                2 => doc.select_word_at(doc.caret),
                n if n >= 3 => select_line(doc, doc.caret),
                _ => {}
            }
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

/// Track repeated `Down` events on the same screen cell and return the click
/// count (1, 2, 3, then wrapping back to 1). Split out from `handle_mouse` so
/// the timing/position logic is unit-testable without a terminal.
fn click_count(app: &mut App, row: u16, col: u16) -> u8 {
    let now = Instant::now();
    let count = match &app.last_click {
        Some(c) if c.row == row && c.col == col && now.duration_since(c.at) < MULTI_CLICK_WINDOW => {
            (c.count % 3) + 1
        }
        _ => 1,
    };
    app.last_click = Some(ClickState { at: now, row, col, count });
    count
}

/// Select the whole source line containing `offset` (the triple-click
/// gesture). `Doc` has no line-selection method, but `anchor`/`caret` are
/// public, so we find the line's byte range the same way the renderer's
/// per-line highlighting does and drive them directly.
fn select_line(doc: &mut Doc, offset: usize) {
    let src = &doc.source;
    let start = src[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = src[offset..].find('\n').map(|i| offset + i).unwrap_or(src.len());
    doc.anchor = Some(start);
    doc.caret = end;
}

/// Copy the current selection to the system clipboard.
fn clipboard_copy(doc: &mut Doc) {
    let Some(text) = doc.selected_text() else {
        doc.status = Some("nothing selected".into());
        return;
    };
    let text = text.to_string();
    doc.status = Some(match set_clipboard_text(text) {
        Ok(()) => "copied".into(),
        Err(_) => "clipboard unavailable".into(),
    });
}

/// Copy the current selection to the system clipboard, then delete it.
fn clipboard_cut(doc: &mut Doc) {
    let Some(text) = doc.selected_text() else {
        doc.status = Some("nothing selected".into());
        return;
    };
    let text = text.to_string();
    match set_clipboard_text(text) {
        Ok(()) => {
            doc.insert(""); // replaces the (still active) selection with nothing
            doc.status = Some("cut".into());
        }
        Err(_) => doc.status = Some("clipboard unavailable".into()),
    }
}

/// Insert the system clipboard's text contents at the caret.
fn clipboard_paste(doc: &mut Doc) {
    match get_clipboard_text() {
        Ok(text) => {
            doc.insert(&text);
            doc.status = Some("pasted".into());
        }
        Err(_) => doc.status = Some("clipboard unavailable".into()),
    }
}

// A fresh `arboard::Clipboard` is opened per call rather than cached on `App`:
// it's cheap, and it sidesteps holding a pasteboard handle stale across focus
// changes. Both helpers collapse arboard's error type so callers only need to
// decide between a status message and a panic (never the latter).

fn set_clipboard_text(text: String) -> Result<(), arboard::Error> {
    arboard::Clipboard::new()?.set_text(text)
}

fn get_clipboard_text() -> Result<String, arboard::Error> {
    arboard::Clipboard::new()?.get_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_click_is_single() {
        let mut app = App::default();
        assert_eq!(click_count(&mut app, 3, 5), 1);
    }

    #[test]
    fn quick_repeat_on_same_cell_advances_to_double_then_triple() {
        let mut app = App::default();
        assert_eq!(click_count(&mut app, 3, 5), 1);
        assert_eq!(click_count(&mut app, 3, 5), 2);
        assert_eq!(click_count(&mut app, 3, 5), 3);
    }

    #[test]
    fn fourth_click_wraps_back_to_single() {
        let mut app = App::default();
        for _ in 0..3 {
            click_count(&mut app, 3, 5);
        }
        assert_eq!(click_count(&mut app, 3, 5), 1);
    }

    #[test]
    fn click_on_a_different_cell_resets_to_single() {
        let mut app = App::default();
        assert_eq!(click_count(&mut app, 3, 5), 1);
        assert_eq!(click_count(&mut app, 3, 5), 2);
        assert_eq!(click_count(&mut app, 4, 5), 1); // different row
        assert_eq!(click_count(&mut app, 4, 6), 1); // different col
    }

    #[test]
    fn stale_click_state_resets_to_single() {
        let mut app = App::default();
        app.last_click = Some(ClickState {
            at: Instant::now() - MULTI_CLICK_WINDOW - Duration::from_millis(1),
            row: 3,
            col: 5,
            count: 2,
        });
        assert_eq!(click_count(&mut app, 3, 5), 1);
    }

    #[test]
    fn select_line_byte_range_covers_middle_line() {
        // `select_line` needs a real `Doc` (constructed from a file on disk),
        // so this exercises the same start/end byte-range math directly.
        let source = "first\nsecond line\nthird";
        let offset = source.find("line").unwrap();
        let start = source[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = source[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(source.len());
        assert_eq!(&source[start..end], "second line");
    }
}
