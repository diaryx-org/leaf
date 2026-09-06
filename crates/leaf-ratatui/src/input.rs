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

use leaf_core::{BlockKind, Doc, InlineKind, LineFlow, MarkupMode, View};

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
    /// ⌥E — the host opens its image-destination prompt.
    ImagePrompt,
    /// The host opens its video-source prompt (no direct key; reached from the
    /// command palette and the Insert menu, since a terminal editor embeds a
    /// movie far less often than it embeds a picture).
    VideoPrompt,
    /// The host opens its audio-source prompt. Palette/menu only, as `VideoPrompt`.
    AudioPrompt,
    /// ⌥P (and ^P) — the host opens the command palette.
    Palette,
    /// ^F — the host opens its find bar.
    Find,
    /// ^H — the host opens its find bar with a replacement field.
    Replace,
    /// ⌥H (and F1) — the host opens the key reference.
    Help,
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
    // A read-only document is a *reading* surface over the same rendering,
    // selection and navigation the editor has — so everything that moves the
    // caret, extends a selection, copies, follows a link, or changes how the
    // document is shown works here exactly as it does anywhere else, and only
    // the keys that would change it stop. See `asks_to_edit` for why the check
    // is in front of the dispatch rather than inside it.
    if doc.read_only() && asks_to_edit(&key) {
        doc.status = Some("read-only".into());
        return Outcome::Continue;
    }
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
            // ^P as well as ⌥p: the palette is the one door to every command
            // that has no key of its own, so it answers to both conventions.
            KeyCode::Char('p') => return Outcome::Palette,
            // ^F find, ^H find-and-replace — the pair everything from a browser
            // to a word processor spells this way. Both were free here, and
            // both arrive as an ordinary `Char` + CONTROL: `\x08` is not one of
            // the bytes crossterm maps to a named key, so ^H does not collide
            // with Backspace (which terminals send as `\x7F`). ⌥h is not
            // available for the replace half — it is the key reference, and has
            // been longer.
            KeyCode::Char('f') => return Outcome::Find,
            KeyCode::Char('h') => return Outcome::Replace,
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
            // Alt+Enter is the in-cell line break. The GUI's chord is Shift+Enter,
            // but a terminal can't tell that from a bare Enter (same byte), so the
            // TUI spells the gesture Alt+Enter, which every terminal reports. Off a
            // table `cell_line_break` declines and we insert an ordinary newline.
            KeyCode::Enter if !doc.cell_line_break() => {
                doc.newline();
            }
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
            // The task pair, beside the list family they belong to: ⌥x ticks the
            // box at the caret (x for the mark it writes), ⌥t gives a plain item
            // a box or takes one away.
            KeyCode::Char('x') => doc.toggle_task_checked(),
            KeyCode::Char('t') => doc.toggle_task_item(),
            KeyCode::Char('v') => return Outcome::PastePlain,
            KeyCode::Char('k') => return Outcome::LinkPrompt,
            KeyCode::Char('l') => return Outcome::LanguagePrompt,
            KeyCode::Char('s') => return Outcome::SaveAs,
            KeyCode::Char('n') => return Outcome::New,
            // The insert family, each on the first letter of what it writes:
            // ⌥f a footnote, ⌥r a rule, ⌥e an embedded picture ("e" because ⌥i
            // is already italic, and a picture is the one embed a terminal
            // actually draws). Video and audio have no key — they're rarer than
            // the keyspace they'd cost — and live in the palette instead.
            KeyCode::Char('f') => doc.insert_footnote(),
            KeyCode::Char('r') => doc.insert_thematic_break(),
            KeyCode::Char('e') => return Outcome::ImagePrompt,
            // ⌥g: follow whatever the caret is standing on — a footnote
            // reference to its note, a note back to its reference, a `#fragment`
            // link to the heading it names.
            KeyCode::Char('g') => follow(doc),
            // The two rendering preferences, shifted onto the keys their
            // unshifted neighbours already own: ⌥w toggles the *view*, so ⌥⇧W
            // cycles how much markup that view reveals; ⌥⇧F flips line flow.
            KeyCode::Char('W') => cycle_markup_mode(doc),
            KeyCode::Char('F') => toggle_line_flow(doc),
            KeyCode::Char('p') => return Outcome::Palette,
            KeyCode::Char('h') => return Outcome::Help,
            _ => {}
        }
        return Outcome::Continue;
    }

    match key.code {
        // F1 is the one help key every terminal agrees on, and the one a reader
        // who has never seen ⌥h will try.
        KeyCode::F(1) => return Outcome::Help,
        KeyCode::Char(c) => doc.insert(&c.to_string()),
        // In a table, Return drops to the cell below, growing the table by a row
        // when there is none — core's `cell_return` policy, shared with the Apple
        // frontend. Off a table it declines and Return is an ordinary newline.
        KeyCode::Enter if doc.cell_return() => {}
        KeyCode::Enter => doc.newline(),
        // In a table, Tab walks the cells (Shift+Tab back) and appends a row when
        // it runs off the last one. Only once the caret isn't in a table does
        // Tab/Shift+Tab fall through to indent/outdent.
        KeyCode::Tab if doc.cell_tab(true) => {}
        KeyCode::BackTab if doc.cell_tab(false) => {}
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

/// Whether this key is asking the document to change — the question the
/// read-only gate has to answer *before* the dispatch below runs, since by the
/// time that has answered it the edit has already happened.
///
/// Core is the guarantee here, not this table. Every mutation funnels through
/// `Doc::splice_exact`/`undo`/`redo`, all three of which refuse outright on a
/// read-only document, so a key this classifier missed still cannot write a
/// byte. What the classifier adds is the two things core's gate can't: a
/// *reason* (core refuses silently, which from the keyboard is indistinguishable
/// from a terminal that has stopped delivering keys), and suppression of the
/// [`Outcome`]s — ^V, ⌥k, ⌥e — that would otherwise have the host open a
/// clipboard read or a destination prompt for an edit that can never land.
///
/// It names the editing keys rather than excluding the reading ones, so a key
/// bound to nothing at all (⌥z) stays silent instead of being refused, and a
/// key added to the dispatch without being added here fails open — into core's
/// gate, which is the one that actually holds.
///
/// The file verbs — ^S, ⌥S, ⌥N — are deliberately *not* here. They change no
/// bytes of the document; what they do is the host's policy about writing
/// files, and the host is where that is stated.
fn asks_to_edit(key: &KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        // Cut, paste, undo/redo, and the two readline kill-lines. ^A, ^C, ^Q,
        // ^P and ^Home/^End are reading gestures and fall through.
        return matches!(
            key.code,
            KeyCode::Char('x' | 'v' | 'z' | 'Z' | 'y' | 'Y' | 'u' | 'k')
        );
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return match key.code {
            // ⌥←/⌥→ are word *motion* and stay; the two word deletes beside
            // them, and the in-cell line break, do not.
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter => true,
            // The formatting toolbar, the block family, the task pair, plain
            // paste, and the three prompts that insert something. What is
            // missing from this list is the whole of the reading half: ⌥w,
            // ⌥⇧W, ⌥⇧F, ⌥g, ⌥p, ⌥h.
            KeyCode::Char(c) => matches!(
                c,
                'b' | 'i' | 'c' | 'm' | 'd' | 'u' | '0'
                    ..='9' | 'x' | 't' | 'v' | 'k' | 'l' | 'e' | 'f' | 'r'
            ),
            _ => false,
        };
    }
    matches!(
        key.code,
        KeyCode::Char(_)
            | KeyCode::Enter
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Backspace
            | KeyCode::Delete
    )
}

/// Apply the caret placement / selection / scroll a mouse event implies,
/// returning the [`MouseOutcome`] the host must act on. Assumes no host overlay
/// is capturing the mouse (the host dismisses its own menu first).
pub fn handle_mouse(doc: &mut Doc, m: MouseEvent, state: &mut EditorState) -> MouseOutcome {
    let (bx, by) = doc.body_origin;
    let within = m.row >= by
        && (m.row as usize) < by as usize + doc.body_height as usize
        && m.column >= bx
        && (doc.body_width == 0 || m.column < bx.saturating_add(doc.body_width));

    // A code row is drawn inset for its box and — if it's the caret's block —
    // scrolled sideways, so a raw screen column has to be shifted back into the
    // block's own column space before it maps to a source byte. Mirrors the
    // draw-time shift in `render`; a plain row is left alone.
    let col_at = |doc: &Doc, state: &EditorState, row: usize, column: u16| -> usize {
        let raw = column.saturating_sub(bx) as usize;
        if doc.view != View::Wysiwyg {
            return raw;
        }
        match doc
            .vmap
            .code_blocks
            .iter()
            .find(|c| c.rows_span.contains(&row))
        {
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

            // A plain click on a rendered checkbox ticks it and does nothing
            // else — the caret stays where it was, so ticking something off a
            // list doesn't interrupt what's being typed elsewhere. Shift and the
            // multi-click gestures fall through: those are selection verbs, and
            // a box is not a selection.
            // …but not in a read-only document, where a box that can't be
            // ticked is just text and a click on it should place the caret
            // like a click on any other glyph.
            if !shift
                && count == 1
                && !doc.read_only()
                && let Some(off) = doc.vmap.task_box_at(row, col)
            {
                doc.toggle_task_at(off);
                return MouseOutcome::Continue;
            }

            // Single click places the caret (extending on shift); double selects
            // the word under it; triple selects the block it's in. All three start
            // from the same `click_at` hit-test so the position → offset mapping
            // lives in one place. The block, not the source line: a paragraph
            // broken over several lines is one paragraph.
            click_at(doc, state, row, col, shift);
            match count {
                2 => doc.select_word_at(doc.caret),
                n if n >= 3 => doc.select_block_at(doc.caret),
                _ => {}
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = col_at(doc, state, row, m.column);
            click_at(doc, state, row, col, true); // extend the selection
        }
        // Dragging past the top or bottom edge of the body scrolls to keep
        // revealing more document. `within`'s column check still applies, but its
        // row check is exactly what these two exist to fall outside of.
        MouseEventKind::Drag(MouseButton::Left) if m.column >= bx && m.row < by => {
            doc.scroll = doc.scroll.saturating_sub(1);
            let col = col_at(doc, state, doc.scroll, m.column);
            click_at(doc, state, doc.scroll, col, true);
        }
        MouseEventKind::Drag(MouseButton::Left)
            if m.column >= bx && (m.row as usize) >= by as usize + doc.body_height as usize =>
        {
            doc.scroll = doc.scroll.saturating_add(1);
            let row = doc.scroll + doc.body_height.saturating_sub(1) as usize;
            let col = col_at(doc, state, row, m.column);
            click_at(doc, state, row, col, true);
        }
        MouseEventKind::Down(MouseButton::Right) if within => {
            // A right-click on top of an existing selection should offer to act
            // on *it*, not collapse it to a fresh caret; approximated with the
            // coarse "is any selection active" since the precise hit-test is
            // private to `Doc`.
            if doc.selection().is_none() {
                let row = doc.scroll + (m.row - by) as usize;
                let col = col_at(doc, state, row, m.column);
                click_at(doc, state, row, col, false);
            }
            return MouseOutcome::ContextMenu {
                x: m.column,
                y: m.row,
            };
        }
        // Pointer over the body with no button down: peek at whatever it's
        // resting on, without moving the caret. See `peek`.
        MouseEventKind::Moved if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = col_at(doc, state, row, m.column);
            peek(doc, state, row, col);
        }
        MouseEventKind::Moved => clear_peek(doc, state),
        MouseEventKind::ScrollDown => doc.scroll = doc.scroll.saturating_add(1),
        MouseEventKind::ScrollUp => doc.scroll = doc.scroll.saturating_sub(1),
        _ => {}
    }
    MouseOutcome::Continue
}

/// Place the caret (or extend the selection) at a mouse position — the one
/// funnel every mouse gesture's caret placement goes through, so none of them
/// can forget the raster case. A position over a painted heading raster is
/// answered by the raster's own layout and lands as a source offset via
/// `Doc::place_caret`; everywhere else the ordinary cell-grid `Doc::click`
/// serves.
#[cfg_attr(not(feature = "images"), allow(unused_variables))]
fn click_at(doc: &mut Doc, state: &mut EditorState, row: usize, col: usize, extend: bool) {
    #[cfg(feature = "images")]
    if let Some(off) = raster_offset(doc, state, row, col) {
        doc.place_caret(off, extend);
        return;
    }
    doc.click(row, col, extend);
}

/// The source offset a pointer position over a *painted* heading raster names,
/// or `None` when it isn't over one. The rasterized glyphs are far wider than
/// the character cells beneath them, so a click mapped through the cell grid
/// would place the caret half a title away from the glyph the pointer visually
/// hit — and with the caret painted into the raster, that miss would now be
/// drawn. The raster's own layout answers instead
/// ([`crate::Images::heading_hit`]), and the character index it returns is a
/// straight lookup in the raster's own char→source table — never a trip
/// through display columns, whose per-cluster widths are core's business, not
/// this module's.
#[cfg(feature = "images")]
fn raster_offset(doc: &Doc, state: &mut EditorState, row: usize, col: usize) -> Option<usize> {
    if doc.view != View::Wysiwyg {
        return None;
    }
    // Split borrows: the raster list is read while the image subsystem (the
    // shaper behind `heading_hit`) is borrowed mutably beside it.
    let EditorState {
        heading_rasters,
        images,
        ..
    } = state;
    let h = heading_rasters
        .iter()
        .find(|h| h.rows_span.contains(&row))?;
    // The raster was painted across the full content width and the span's rows.
    let cells = (doc.body_width, h.rows_span.len() as u16);
    let pos = (
        col.min(u16::MAX as usize) as u16,
        (row - h.rows_span.start) as u16,
    );
    let idx = images.heading_hit(&h.text, h.level, cells, pos);
    // The `idx`-th character's source offset; past the end — a click in the
    // clear space right of the title — lands after the last glyph, exactly
    // where End would put the caret.
    Some(h.srcs.get(idx).copied().unwrap_or(h.end_src))
}

/// Show what the pointer is resting on, without disturbing where anyone is
/// typing — a footnote reference resolved to its note, or a link resolved to its
/// destination.
///
/// This is what core's *offset-based* `footnote_at` and `link_destination_at`
/// are for, as against their `_at_caret` siblings: the gesture that most wants
/// to know what a `[1]` names is precisely the one that must not move the caret
/// to find out. A reader hovering a reference mid-sentence is asking a question
/// about the document, not editing it.
///
/// Only in the rich view: the source view already shows `[^1]` and the URL in
/// full, so there is nothing hidden to reveal.
///
/// The peek is published once, when the pointer arrives on a target, rather than
/// on every mouse-move a terminal sends while crossing a word — and it is taken
/// back down only if *this* is what put it up, so hovering past a footnote can't
/// wipe out a "saved" or "clipboard unavailable" the user still needs to read.
fn peek(doc: &mut Doc, state: &mut EditorState, row: usize, col: usize) {
    if doc.view != View::Wysiwyg {
        return clear_peek(doc, state);
    }
    // Over a painted heading raster the offset comes from the raster's layout,
    // like a click's would — a link inside a big title peeks at the words the
    // pointer is visually on.
    #[cfg(feature = "images")]
    let off =
        raster_offset(doc, state, row, col).unwrap_or_else(|| doc.vmap.offset_of_pos(row, col));
    #[cfg(not(feature = "images"))]
    let off = doc.vmap.offset_of_pos(row, col);
    if state.peek == Some(off) {
        return; // already showing this one
    }
    if let Some(note) = doc.footnote_at(off) {
        // The note's own words, trimmed to a line — a peek is a glance, and a
        // footnote that runs to a paragraph would otherwise push a paragraph
        // into a one-row toast.
        let body = note.text.unwrap_or_else(|| "(no note)".into());
        doc.status = Some(format!("[{}] {}", note.label, summarize(&body)));
        state.peek = Some(off);
    } else if let Some(dest) = doc.link_destination_at(off) {
        doc.status = Some(format!("→ {}", summarize(&dest)));
        state.peek = Some(off);
    } else {
        clear_peek(doc, state);
    }
}

/// Take down a peek this module put up, and only that. A status somebody else
/// set is left alone — the pointer wandering across the document is not an
/// event that should be able to clear a message about a failed save.
fn clear_peek(doc: &mut Doc, state: &mut EditorState) {
    if state.peek.take().is_some() {
        doc.status = None;
    }
}

/// One line's worth of `text`, with newlines folded to spaces and a tail cut off
/// at an ellipsis. The peek shares the status toast with everything else, and
/// the toast is one row.
fn summarize(text: &str) -> String {
    const LIMIT: usize = 60;
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LIMIT {
        return flat;
    }
    flat.chars().take(LIMIT - 1).collect::<String>() + "…"
}

/// ⌥g — follow whatever the caret is standing on, without leaving the document.
///
/// Three things in a document point somewhere, and all three are *in-document*
/// moves, which is why one key covers them: a footnote reference points at its
/// note, a note points back at the reference that cites it, and a `#fragment`
/// link points at the block whose id or heading text it names (`Doc::locate`,
/// the rule every Markdown renderer already follows). The caret lands on the
/// target; nothing is opened and nothing is spawned.
///
/// An *external* destination is reported rather than followed. Opening one means
/// launching a browser out of a text editor, which is a decision for the person
/// at the keyboard and not for a keystroke that also means "jump to a footnote";
/// most terminals already make a printed URL clickable, so saying what the link
/// points at is the useful half and the safe one.
///
/// The order is reference → definition → link, which is the order of how
/// specific the thing under the caret is: a `[^1]` is unambiguous, a note body
/// is the only other place a footnote round trip can start, and a link is
/// whatever is left.
pub fn follow(doc: &mut Doc) {
    // A footnote reference: go to the note it names.
    if let Some(note) = doc.footnote_at_caret() {
        match note.offset {
            Some(off) => {
                doc.place_caret(off, false);
                doc.status = Some(format!("note [{}]", note.label));
            }
            // A reference with no definition to read: worth saying so, since the
            // silence would otherwise read as a key that did nothing.
            None => doc.status = Some(format!("[^{}] has no note", note.label)),
        }
        return;
    }
    // Inside a note: go back to the reference that cites it.
    if let Some(def) = doc.footnote_definition_at_caret() {
        match def.offset {
            Some(off) => {
                doc.place_caret(off, false);
                doc.status = Some(format!("back to [^{}]", def.label));
            }
            None => doc.status = Some(format!("[^{}] is never cited", def.label)),
        }
        return;
    }
    // A link: a fragment lands in this document, anything else is reported.
    if let Some(dest) = doc.link_destination_at_caret() {
        match dest.strip_prefix('#') {
            Some(fragment) => match doc.locate(fragment) {
                Some(landing) => {
                    doc.place_caret(landing.start, false);
                    doc.status = Some(format!("#{fragment}"));
                }
                None => doc.status = Some(format!("no #{fragment} in this document")),
            },
            None => doc.status = Some(format!("→ {dest}")),
        }
        return;
    }
    doc.status = Some("nothing to follow here".into());
}

/// ⌥⇧W — cycle how much markup the rich view reveals: none → shortcuts → full →
/// none. A cycle rather than three separate commands because the three are one
/// dial, and a dial with three notches is quicker to *turn* than to aim at; the
/// status names the notch it landed on, so the key teaches its own range.
pub fn cycle_markup_mode(doc: &mut Doc) {
    let next = match doc.markup_mode() {
        MarkupMode::None => MarkupMode::Shortcuts,
        MarkupMode::Shortcuts => MarkupMode::Full,
        MarkupMode::Full => MarkupMode::None,
    };
    doc.set_markup_mode(next);
    doc.status = Some(format!("markup: {}", markup_mode_name(next)));
}

/// ⌥⇧F — flip between folding soft breaks into the reflowed paragraph and
/// preserving them where they were written.
pub fn toggle_line_flow(doc: &mut Doc) {
    let next = match doc.line_flow() {
        LineFlow::Fold => LineFlow::Preserve,
        LineFlow::Preserve => LineFlow::Fold,
    };
    doc.set_line_flow(next);
    doc.status = Some(format!("line flow: {}", line_flow_name(next)));
}

/// The word for a [`MarkupMode`], for a status line or a menu row. Deliberately
/// the reader's word rather than the variant's: `None` is a mode in which markup
/// is hidden, and "none" alone reads like the absence of a setting.
pub fn markup_mode_name(mode: MarkupMode) -> &'static str {
    match mode {
        MarkupMode::None => "hidden",
        MarkupMode::Shortcuts => "shortcuts",
        MarkupMode::Full => "full",
    }
}

/// The word for a [`LineFlow`], for the same two places as [`markup_mode_name`].
pub fn line_flow_name(flow: LineFlow) -> &'static str {
    match flow {
        LineFlow::Fold => "fold",
        LineFlow::Preserve => "preserve",
    }
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
    let kind = if ordered {
        "ordered_list"
    } else {
        "bullet_list"
    };
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
        Some(c)
            if c.row == row && c.col == col && now.duration_since(c.at) < MULTI_CLICK_WINDOW =>
        {
            (c.count % 3) + 1
        }
        _ => 1,
    };
    state.last_click = Some(ClickState {
        at: now,
        row,
        col,
        count,
    });
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A Markdown document with its visual map built, which caret motion in the
    /// rich view rides — an unbuilt map has no rows to move through.
    fn doc(body: &str) -> Doc {
        let mut d = Doc::from_source(body.into(), leaf_core::Format::Markdown).unwrap();
        d.build_visual(80);
        d
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    /// ⌥m and ⌥d over a Markdown selection, end to end from the key. The two
    /// marks twig 3.3.1 made authorable there — before it, both keys reached
    /// core and came back with a status line instead of a mark, which is a
    /// keyboard that looks broken.
    #[test]
    fn alt_m_and_alt_d_mark_the_selection_in_markdown() {
        for (key, marked) in [('m', "a ==word== b\n"), ('d', "a ~~word~~ b\n")] {
            let mut d = doc("a word b\n");
            let mut state = EditorState::new();
            d.anchor = Some(2);
            d.caret = 6;
            handle_key(&mut d, alt(key), &mut state);
            assert_eq!(d.source, marked, "⌥{key}");
            assert_eq!(d.status, None, "⌥{key} should have nothing to explain");
        }
    }

    /// Every key that would change the document is refused *with a reason*.
    /// Core's gate already makes them harmless; what this is checking is that
    /// they don't read as a keyboard that has quietly stopped working.
    #[test]
    fn a_read_only_document_refuses_the_editing_keys_and_says_so() {
        let mut d = doc("hello world\n");
        d.set_read_only(true);
        d.caret = 5;
        let mut state = EditorState::new();
        let refused = [
            plain(KeyCode::Char('x')),
            plain(KeyCode::Enter),
            plain(KeyCode::Tab),
            plain(KeyCode::Backspace),
            plain(KeyCode::Delete),
            ctrl('x'),
            ctrl('v'),
            ctrl('z'),
            ctrl('u'),
            ctrl('k'),
            alt('b'),
            alt('1'),
            alt('k'),
            alt('e'),
            alt('v'),
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
        ];
        for key in refused {
            d.status = None;
            assert_eq!(
                handle_key(&mut d, key, &mut state),
                Outcome::Continue,
                "{key:?} should not reach the host"
            );
            assert_eq!(
                d.status.as_deref(),
                Some("read-only"),
                "{key:?} should have said why"
            );
        }
        assert_eq!(d.source, "hello world\n", "and not a byte moved");
    }

    /// The other half, and the point of the mode: reading a document is not a
    /// degraded form of editing it.
    #[test]
    fn a_read_only_document_still_navigates_selects_and_copies() {
        let mut d = doc("hello world\n");
        d.set_read_only(true);
        let mut state = EditorState::new();

        handle_key(&mut d, plain(KeyCode::Right), &mut state);
        assert_eq!(d.caret, 1, "the caret moves");
        handle_key(
            &mut d,
            KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT),
            &mut state,
        );
        assert_eq!(d.selected_text(), Some("e"), "selection extends");
        assert_eq!(handle_key(&mut d, ctrl('c'), &mut state), Outcome::Copy);
        assert_eq!(handle_key(&mut d, ctrl('a'), &mut state), Outcome::Continue);
        assert_eq!(d.selected_text(), Some("hello world\n"), "^a selects all");
        assert_eq!(handle_key(&mut d, ctrl('p'), &mut state), Outcome::Palette);
        assert_eq!(handle_key(&mut d, alt('h'), &mut state), Outcome::Help);
        assert_eq!(handle_key(&mut d, ctrl('q'), &mut state), Outcome::Quit);

        // The view dials are about how the document is shown, not what it says.
        let before = d.view;
        handle_key(&mut d, alt('w'), &mut state);
        assert_ne!(d.view, before, "⌥w still toggles the view");
    }

    /// The file verbs are the host's policy, not the widget's, so they reach it
    /// and it decides — see `attempt_save` in leaf-tui.
    #[test]
    fn the_file_verbs_still_reach_the_host_to_be_refused_there() {
        let mut d = doc("hello\n");
        d.set_read_only(true);
        let mut state = EditorState::new();
        assert_eq!(handle_key(&mut d, ctrl('s'), &mut state), Outcome::Save);
        assert_eq!(handle_key(&mut d, alt('s'), &mut state), Outcome::SaveAs);
        assert_eq!(handle_key(&mut d, alt('n'), &mut state), Outcome::New);
    }

    /// A key bound to nothing must not be answered with a refusal — that would
    /// invent a message where the editable document is silent.
    #[test]
    fn an_unbound_key_is_still_silent_in_a_read_only_document() {
        let mut d = doc("hello\n");
        d.set_read_only(true);
        let mut state = EditorState::new();
        handle_key(&mut d, alt('z'), &mut state);
        assert_eq!(d.status, None);
    }

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
