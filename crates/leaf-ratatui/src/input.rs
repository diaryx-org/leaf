//! Turning crossterm key/mouse events into `leaf_core::Doc` edits.
//!
//! The editing surface handles everything that mutates the document directly —
//! caret motion, insertion, mark/heading/list toggles, undo/redo, selection by
//! click and drag. Anything the *host* owns — quitting, saving, the clipboard,
//! opening a prompt or a context menu — is not done here; it's named in the
//! returned [`Outcome`] / [`MouseOutcome`] for the host to carry out. The host is
//! also responsible for intercepting its own modal overlays (dialogs, menus)
//! before it ever forwards an event to these functions.

use std::time::Instant;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use leaf_core::{BlockKind, Doc, InlineKind, View};

use crate::style::CODE_INSET;
use crate::{ClickState, EditorState, MULTI_CLICK_WINDOW};

/// What the host must do after the editor has handled a key. `Continue` means
/// the key was fully handled internally (the host just redraws); every other
/// variant is an action the host owns — the editor deliberately doesn't touch
/// the terminal, the filesystem, the clipboard, or its own dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Handled by the editor (or ignored). Redraw and read the next event.
    Continue,
    /// Ctrl+Q — the host quits (guarding an unsaved document as it sees fit).
    Quit,
    /// Ctrl+S — the host saves (its own untitled/conflict handling).
    Save,
    /// ⌥S — the host runs its "save as" flow.
    SaveAs,
    /// ⌥N — the host swaps in a new document (guarding unsaved changes).
    New,
    /// Ctrl+C — the host copies the selection to the system clipboard.
    Copy,
    /// Ctrl+X — the host cuts the selection to the system clipboard.
    Cut,
    /// Ctrl+V — the host pastes the clipboard's rich flavor (falling back to plain).
    Paste,
    /// ⌥V — the host pastes the clipboard's plain flavor.
    PastePlain,
    /// ⌥K — the host opens its link-destination prompt.
    LinkPrompt,
    /// ⌥L — the host opens its code-language prompt.
    LanguagePrompt,
}

/// What the host must do after the editor has handled a mouse event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseOutcome {
    /// Handled by the editor (caret placement, selection, scroll) — just redraw.
    Continue,
    /// Right-click — the host opens its context menu anchored at this screen cell.
    ContextMenu { x: u16, y: u16 },
}

/// Apply the edit a key implies, returning the [`Outcome`] the host must act on.
/// Assumes no host overlay (dialog/menu/prompt) is currently capturing input —
/// the host intercepts those before forwarding here.
pub fn handle_key(doc: &mut Doc, key: KeyEvent, _state: &mut EditorState) -> Outcome {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if ctrl {
        match key.code {
            KeyCode::Char('q') => return Outcome::Quit,
            KeyCode::Char('s') => return Outcome::Save,
            KeyCode::Char('a') => doc.select_all(),
            KeyCode::Char('c') => return Outcome::Copy,
            KeyCode::Char('x') => return Outcome::Cut,
            KeyCode::Char('v') => return Outcome::Paste,
            // ^Z undo, ^⇧Z or ^Y redo.
            KeyCode::Char('z') | KeyCode::Char('Z') if shift => doc.redo(),
            KeyCode::Char('z') | KeyCode::Char('Z') => doc.undo(),
            KeyCode::Char('y') | KeyCode::Char('Y') => doc.redo(),
            // Readline's kill-line pair: ^U back to the line start, ^K forward to
            // its end — the convention a terminal user already has under their
            // fingers.
            KeyCode::Char('u') => doc.delete_to_line_start(),
            KeyCode::Char('k') => doc.delete_to_line_end(),
            // ^Home / ^End jump to the document's start / end.
            KeyCode::Home => doc.move_doc_start(shift),
            KeyCode::End => doc.move_doc_end(shift),
            _ => {}
        }
        return Outcome::Continue;
    }

    if alt {
        // The formatting toolbar. Inline marks act on the selection; heading /
        // body conversion acts on the block at the caret. Word motion/delete
        // share this modifier since crossterm reports Alt+Left/Right/Backspace/
        // Delete as ordinary key codes plus ALT.
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
            // twig models strikethrough/underline as the Delete/Insert marks
            // (their names in the CommonMark/Djot extensions that define them),
            // matching ⌥d/⌥u to what a user reads: struck-through and underlined.
            KeyCode::Char('d') => doc.toggle(InlineKind::Delete),
            KeyCode::Char('u') => doc.toggle(InlineKind::Insert),
            KeyCode::Char('0') => doc.set_block(BlockKind::Paragraph),
            // Toggle, not set: ⌥1 on a line that's already H1 reverts it to a
            // paragraph, matching the feel of the bold/italic/code toggles.
            KeyCode::Char(d @ '1'..='6') => doc.toggle_heading(d.to_digit(10).unwrap()),
            // Headings stop at 6, so the numeric family keeps going: ⌥7/⌥8 are
            // the numbered/bulleted pair, ⌥9 is quote.
            KeyCode::Char('7') => toggle_list(doc, true),
            KeyCode::Char('8') => toggle_list(doc, false),
            KeyCode::Char('9') => doc.toggle_blockquote(),
            KeyCode::Char('v') => return Outcome::PastePlain,
            KeyCode::Char('k') => return Outcome::LinkPrompt,
            KeyCode::Char('l') => return Outcome::LanguagePrompt,
            KeyCode::Char('s') => return Outcome::SaveAs,
            KeyCode::Char('n') => return Outcome::New,
            _ => {}
        }
        return Outcome::Continue;
    }

    match key.code {
        KeyCode::Char(c) => doc.insert(&c.to_string()),
        KeyCode::Enter => doc.newline(),
        // In a table, Tab walks the cells (Shift+Tab back). Only once the caret
        // isn't in a table does Tab/Shift+Tab fall through to indent/outdent.
        KeyCode::Tab if doc.cell_hop(true) => {}
        KeyCode::BackTab if doc.cell_hop(false) => {}
        KeyCode::Tab => doc.indent(),
        KeyCode::BackTab => doc.outdent(),
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
    Outcome::Continue
}

/// Apply the caret placement / selection / scroll a mouse event implies,
/// returning the [`MouseOutcome`] the host must act on. Assumes no host overlay
/// is capturing the mouse (the host dismisses its own menu first).
pub fn handle_mouse(doc: &mut Doc, m: MouseEvent, state: &mut EditorState) -> MouseOutcome {
    let (bx, by) = doc.body_origin;
    let within = m.row >= by
        && (m.row as usize) < by as usize + doc.body_height as usize
        && m.column >= bx;

    // A code row is drawn inset for its box and — if it's the caret's block —
    // scrolled sideways, so a raw screen column has to be shifted back into the
    // block's own column space before it maps to a source byte. Mirrors the
    // draw-time shift in `render`; a plain row is left alone.
    let col_at = |doc: &Doc, state: &EditorState, row: usize, column: u16| -> usize {
        let raw = column.saturating_sub(bx) as usize;
        if doc.view != View::Wysiwyg {
            return raw;
        }
        match doc.vmap.code_blocks.iter().find(|c| c.rows_span.contains(&row)) {
            Some(cb) => {
                let scroll = if state.code_caret_span.as_ref() == Some(&cb.rows_span) {
                    state.code_scroll_x
                } else {
                    0
                };
                raw.saturating_sub(CODE_INSET) + scroll
            }
            None => raw,
        }
    };

    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = col_at(doc, state, row, m.column);
            let count = click_count(state, m.row, m.column);
            let shift = m.modifiers.contains(KeyModifiers::SHIFT);

            // Single click places the caret (extending on shift); double selects
            // the word under it; triple selects the block it's in. All three start
            // from the same `click` hit-test so the row/col → offset mapping lives
            // in one place. The block, not the source line: a paragraph broken over
            // several lines is one paragraph.
            doc.click(row, col, shift);
            match count {
                2 => doc.select_word_at(doc.caret),
                n if n >= 3 => doc.select_block_at(doc.caret),
                _ => {}
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = col_at(doc, state, row, m.column);
            doc.click(row, col, true); // extend the selection
        }
        // Dragging past the top or bottom edge of the body scrolls to keep
        // revealing more document. `within`'s column check still applies, but its
        // row check is exactly what these two exist to fall outside of.
        MouseEventKind::Drag(MouseButton::Left) if m.column >= bx && m.row < by => {
            doc.scroll = doc.scroll.saturating_sub(1);
            let col = col_at(doc, state, doc.scroll, m.column);
            doc.click(doc.scroll, col, true);
        }
        MouseEventKind::Drag(MouseButton::Left)
            if m.column >= bx && (m.row as usize) >= by as usize + doc.body_height as usize =>
        {
            doc.scroll = doc.scroll.saturating_add(1);
            let row = doc.scroll + doc.body_height.saturating_sub(1) as usize;
            let col = col_at(doc, state, row, m.column);
            doc.click(row, col, true);
        }
        MouseEventKind::Down(MouseButton::Right) if within => {
            // A right-click on top of an existing selection should offer to act
            // on *it*, not collapse it to a fresh caret; approximated with the
            // coarse "is any selection active" since the precise hit-test is
            // private to `Doc`.
            if doc.selection().is_none() {
                let row = doc.scroll + (m.row - by) as usize;
                let col = col_at(doc, state, row, m.column);
                doc.click(row, col, false);
            }
            return MouseOutcome::ContextMenu { x: m.column, y: m.row };
        }
        MouseEventKind::ScrollDown => doc.scroll = doc.scroll.saturating_add(1),
        MouseEventKind::ScrollUp => doc.scroll = doc.scroll.saturating_sub(1),
        _ => {}
    }
    MouseOutcome::Continue
}

/// The page step: the body's visible rows minus one for overlap (at least one).
fn page_rows(doc: &Doc) -> usize {
    (doc.body_height as usize).saturating_sub(1).max(1)
}

/// ⌥7/⌥8: toggle an ordered/bulleted list, then check whether that just nested
/// rather than un-listed. `Doc::toggle_list` un-wraps a container only when the
/// edited range covers every block it holds; a bare caret's range is just its
/// own block, so pressing the same list's key a second time inside a multi-item
/// list nests instead of undoing. What this can do is stop the nest from reading
/// as "nothing happened": the breadcrumb's count of `kind` ancestors goes up,
/// exactly when that's what occurred, so that's the signal the status hangs off.
fn toggle_list(doc: &mut Doc, ordered: bool) {
    let kind = if ordered { "ordered_list" } else { "bullet_list" };
    let no_selection = doc.selection().is_none();
    let before = list_depth(doc, kind);
    doc.toggle_list(ordered);
    if no_selection && doc.status.is_none() && list_depth(doc, kind) > before {
        doc.status = Some("nested — select the whole list to un-list it".into());
    }
}

/// How many `kind` ancestors wrap the caret, read off the same breadcrumb the
/// header displays — the only public window onto AST ancestry a frontend has.
fn list_depth(doc: &mut Doc, kind: &str) -> usize {
    doc.breadcrumb().split(" › ").filter(|k| *k == kind).count()
}

/// Track repeated `Down` events on the same screen cell and return the click
/// count (1, 2, 3, then wrapping back to 1). Split out so the timing/position
/// logic is unit-testable without a terminal.
fn click_count(state: &mut EditorState, row: u16, col: u16) -> u8 {
    let now = Instant::now();
    let count = match &state.last_click {
        Some(c) if c.row == row && c.col == col && now.duration_since(c.at) < MULTI_CLICK_WINDOW => {
            (c.count % 3) + 1
        }
        _ => 1,
    };
    state.last_click = Some(ClickState { at: now, row, col, count });
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn first_click_is_single() {
        let mut state = EditorState::new();
        assert_eq!(click_count(&mut state, 3, 5), 1);
    }

    #[test]
    fn quick_repeat_on_same_cell_advances_to_double_then_triple() {
        let mut state = EditorState::new();
        assert_eq!(click_count(&mut state, 3, 5), 1);
        assert_eq!(click_count(&mut state, 3, 5), 2);
        assert_eq!(click_count(&mut state, 3, 5), 3);
    }

    #[test]
    fn fourth_click_wraps_back_to_single() {
        let mut state = EditorState::new();
        for _ in 0..3 {
            click_count(&mut state, 3, 5);
        }
        assert_eq!(click_count(&mut state, 3, 5), 1);
    }

    #[test]
    fn click_on_a_different_cell_resets_to_single() {
        let mut state = EditorState::new();
        assert_eq!(click_count(&mut state, 3, 5), 1);
        assert_eq!(click_count(&mut state, 3, 5), 2);
        assert_eq!(click_count(&mut state, 4, 5), 1); // different row
        assert_eq!(click_count(&mut state, 4, 6), 1); // different col
    }

    #[test]
    fn stale_click_state_resets_to_single() {
        let mut state = EditorState::new();
        state.last_click = Some(ClickState {
            at: Instant::now() - MULTI_CLICK_WINDOW - Duration::from_millis(1),
            row: 3,
            col: 5,
            count: 2,
        });
        assert_eq!(click_count(&mut state, 3, 5), 1);
    }
}
