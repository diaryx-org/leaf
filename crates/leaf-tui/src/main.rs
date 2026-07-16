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
use ratatui::{
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
            KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        },
        execute,
    },
    layout::Rect,
};
use leaf_core::{BlockKind, DiskState, Doc, InlineKind};

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

/// UI-only state that doesn't belong on `Doc`: the quit-confirmation prompt,
/// mouse click-counting for double/triple-click, the right-click context menu,
/// and the source view's horizontal scroll. Doc stays the frontend-neutral
/// model; this is the crossterm-facing bookkeeping around it.
#[derive(Default)]
struct App {
    /// Set by Ctrl+Q or ⌥n meeting a dirty document: the footer shows a
    /// Save/Discard/Cancel choice and normal key handling is suspended until
    /// one is picked. What runs once it's picked (and, for Save, once any
    /// dialog that choice opens resolves) is `dirty_prompt`'s own `action`.
    dirty_prompt: Option<DirtyPrompt>,
    /// What a resolved `dirty_prompt`'s Save choice is waiting to do once the
    /// document comes out clean — `None` for a bare ^S, which has nothing to
    /// do once it's saved. Set right before whichever dialog a save has to
    /// open first (Save As for an untitled document, the overwrite/reload
    /// choice for a conflict) and consumed by `resolve_pending` once that
    /// dialog resolves, so a Save chosen from the quit prompt still quits
    /// after a Save-As detour, and one chosen to guard ⌥n still swaps in the
    /// blank document after it.
    pending_action: Option<DirtyAction>,
    /// Set when a save is about to write over a file that changed on disk
    /// since leaf last read or wrote it (`Doc::disk_state`); offers
    /// Overwrite/Reload/Cancel instead of silently clobbering someone else's
    /// edit. See `attempt_save` for the one place that sets it.
    conflict: Option<ConflictPrompt>,
    /// Timing and screen cell of the last left mouse-down, for detecting
    /// double/triple clicks.
    last_click: Option<ClickState>,
    /// Present while the right-click menu is open; consumes keyboard and
    /// mouse input until an item is chosen or it's dismissed.
    context_menu: Option<ContextMenu>,
    /// Present while a single-line input (the link-destination prompt today,
    /// Save As later) is open; consumes the keyboard the same way
    /// `context_menu` does, until Enter confirms or Esc cancels it.
    text_prompt: Option<TextPrompt>,
    /// How far the source view is scrolled sideways. There's no horizontal
    /// scroll wheel to drive this independently (unlike `doc.scroll`), so it
    /// only ever chases the caret — see `ui::follow_caret_x`.
    scroll_x: usize,
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

/// The right-click menu's rows, in display order: a label paired with the
/// action a click or Enter on that row runs. `ui::render_context_menu` reads
/// the labels off this same list so the menu drawn on screen and the actions
/// wired to it can't drift apart.
const MENU_ITEMS: &[(&str, fn(&mut Doc))] = &[
    ("Cut", clipboard_cut),
    ("Copy", clipboard_copy),
    ("Paste", clipboard_paste),
    ("Select All", Doc::select_all),
];

struct ContextMenu {
    /// Screen cell the right-click landed on; the overlay is anchored here
    /// (and nudged back on screen if it wouldn't fit).
    anchor: (u16, u16),
    /// Index into `MENU_ITEMS` currently highlighted, moved by the arrow keys.
    selected: usize,
    /// The rect `ui::render_context_menu` last painted the menu at, stashed
    /// the same way `doc.body_origin`/`body_height` are, so mouse hit-testing
    /// here and drawing there agree on one geometry.
    rect: Option<Rect>,
}

/// A minimal, reusable single-line input: a label, a starting value, and a
/// callback to run on confirm. Modeled on `ContextMenu` — state lives on
/// `App`, `ui::render_text_prompt` paints it — but there's nothing here to
/// hit-test (no rows to click), so unlike the menu it stashes no rect back.
struct TextPrompt {
    label: &'static str,
    value: String,
    /// Byte offset into `value`; only ever moved by whole `char`s, so always
    /// on a UTF-8 boundary.
    cursor: usize,
    on_confirm: fn(&mut Doc, &str),
}

impl TextPrompt {
    fn new(label: &'static str, initial: impl Into<String>, on_confirm: fn(&mut Doc, &str)) -> Self {
        let value = initial.into();
        let cursor = value.len();
        TextPrompt { label, value, cursor, on_confirm }
    }
}

/// What a `DirtyPrompt` is guarding: quitting, or replacing the buffer with a
/// new blank document. Both walk away from whatever's in `doc` right now, so
/// both need the same Save/Discard/Cancel choice before losing it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DirtyAction {
    Quit,
    New,
}

/// The Save/Discard/Cancel choice offered in place of the old y/n "quit
/// without saving?" — same overlay-owns-the-keyboard shape as `ContextMenu`,
/// but, like `TextPrompt`, drawn inline in the footer rather than floated:
/// there's no click to anchor it to.
struct DirtyPrompt {
    action: DirtyAction,
    /// Index into `["Save", "Discard", "Cancel"]`, moved by the arrow keys;
    /// `s`/`d`/`c` jump straight to a choice the way they always could.
    /// Defaults to Save — the one an accidental Enter should do, on the same
    /// reasoning every "unsaved changes" dialog defaults to it.
    selected: usize,
}

/// The Overwrite/Reload/Cancel choice offered when a save is about to write
/// over a file that changed on disk since leaf last touched it. Shaped like
/// `DirtyPrompt` for the same reason.
struct ConflictPrompt {
    /// Defaults to Cancel — unlike `DirtyPrompt`, the risky option here
    /// (Overwrite, clobbering someone else's edit) is *not* what an
    /// accidental Enter should do.
    selected: usize,
}

fn run(terminal: &mut ratatui::DefaultTerminal, doc: &mut Doc) -> Result<()> {
    let mut app = App::default();
    loop {
        let breadcrumb = doc.breadcrumb();
        terminal.draw(|f| ui::render(f, doc, &breadcrumb, &mut app))?;

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
    // The Save/Discard/Cancel prompt takes over the keyboard until answered,
    // the same as the old y/n quit confirmation did — but Save can lead
    // through a Save-As detour (untitled) or a conflict check (see
    // `attempt_save`), so unlike the old bool this doesn't always resolve in
    // one keystroke.
    if let Some(prompt) = &mut app.dirty_prompt {
        match key.code {
            KeyCode::Up => prompt.selected = (prompt.selected + 3 - 1) % 3,
            KeyCode::Down => prompt.selected = (prompt.selected + 1) % 3,
            KeyCode::Char('s') | KeyCode::Char('S') => return resolve_dirty_prompt(doc, app, 0),
            KeyCode::Char('d') | KeyCode::Char('D') => return resolve_dirty_prompt(doc, app, 1),
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => app.dirty_prompt = None,
            KeyCode::Enter => {
                let selected = prompt.selected;
                return resolve_dirty_prompt(doc, app, selected);
            }
            _ => {} // anything else: leave the prompt up
        }
        return Flow::Continue;
    }

    // The overwrite/reload conflict prompt, same shape as `dirty_prompt`.
    if let Some(prompt) = &mut app.conflict {
        match key.code {
            KeyCode::Up => prompt.selected = (prompt.selected + 3 - 1) % 3,
            KeyCode::Down => prompt.selected = (prompt.selected + 1) % 3,
            KeyCode::Char('o') | KeyCode::Char('O') => return resolve_conflict(doc, app, 0),
            KeyCode::Char('r') | KeyCode::Char('R') => return resolve_conflict(doc, app, 1),
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                app.conflict = None;
                app.pending_action = None;
            }
            KeyCode::Enter => {
                let selected = prompt.selected;
                return resolve_conflict(doc, app, selected);
            }
            _ => {}
        }
        return Flow::Continue;
    }

    // The context menu takes over the keyboard the same way the prompts above
    // do: arrows move the highlight, Enter runs the highlighted row, Esc (or
    // anything else) closes it without acting.
    if let Some(menu) = &mut app.context_menu {
        match key.code {
            KeyCode::Up => menu.selected = (menu.selected + MENU_ITEMS.len() - 1) % MENU_ITEMS.len(),
            KeyCode::Down => menu.selected = (menu.selected + 1) % MENU_ITEMS.len(),
            KeyCode::Enter => {
                let action = MENU_ITEMS[menu.selected].1;
                app.context_menu = None;
                action(doc);
            }
            _ => app.context_menu = None,
        }
        return Flow::Continue;
    }

    // The text prompt takes the keyboard over completely — every code below
    // this, including ^-save and ⌥-formatting, must not leak through to the
    // document while it's up, or a save-as destination could double as a
    // formatting command on the document underneath.
    if let Some(prompt) = &mut app.text_prompt {
        match key.code {
            KeyCode::Backspace => {
                if let Some((i, _)) = prompt.value[..prompt.cursor].char_indices().next_back() {
                    prompt.value.drain(i..prompt.cursor);
                    prompt.cursor = i;
                }
            }
            KeyCode::Left => {
                if let Some((i, _)) = prompt.value[..prompt.cursor].char_indices().next_back() {
                    prompt.cursor = i;
                }
            }
            KeyCode::Right => {
                if let Some(c) = prompt.value[prompt.cursor..].chars().next() {
                    prompt.cursor += c.len_utf8();
                }
            }
            KeyCode::Char(c) => {
                prompt.value.insert(prompt.cursor, c);
                prompt.cursor += c.len_utf8();
            }
            KeyCode::Enter => {
                // Pull the value and callback out before dropping the prompt —
                // same "read what's needed, then clear" order the context menu
                // uses to run its highlighted action, so `on_confirm` sees a
                // `doc` with no prompt left standing over it.
                let value = std::mem::take(&mut prompt.value);
                let on_confirm = prompt.on_confirm;
                app.text_prompt = None;
                on_confirm(doc, &value);
                // A Save-As opened by `attempt_save` leaves a `pending_action`
                // behind for exactly this moment: the link prompt has none, so
                // this is a no-op there.
                return resolve_pending(doc, app);
            }
            KeyCode::Esc => {
                app.text_prompt = None;
                // Whatever Save flow opened this (quit/new's Save choice, or a
                // conflict's overwrite) is abandoned, not retried — the user
                // backed out of naming a file, not of the choice to save.
                app.pending_action = None;
            }
            _ => {}
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
                    app.dirty_prompt = Some(DirtyPrompt { action: DirtyAction::Quit, selected: 0 });
                } else {
                    return Flow::Quit;
                }
            }
            KeyCode::Char('s') => {
                attempt_save(doc, app, None);
            }
            KeyCode::Char('a') => doc.select_all(),
            KeyCode::Char('c') => clipboard_copy(doc),
            KeyCode::Char('x') => clipboard_cut(doc),
            KeyCode::Char('v') => clipboard_paste(doc),
            // ^Z undo, ^⇧Z or ^Y redo.
            KeyCode::Char('z') | KeyCode::Char('Z') if shift => doc.redo(),
            KeyCode::Char('z') | KeyCode::Char('Z') => doc.undo(),
            KeyCode::Char('y') | KeyCode::Char('Y') => doc.redo(),
            // Readline's kill-line pair: ^U back to the line start, ^K forward
            // to its end — the convention a terminal user already has under
            // their fingers, and free (neither was bound to anything here).
            KeyCode::Char('u') => doc.delete_to_line_start(),
            KeyCode::Char('k') => doc.delete_to_line_end(),
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
            // twig models strikethrough/underline as the Delete/Insert marks
            // (their names in the CommonMark/Djot extensions that define them,
            // not the toolbar's), matching ⌥d/⌥u to what a user actually reads
            // in the WYSIWYG view: struck-through and underlined text.
            KeyCode::Char('d') => doc.toggle(InlineKind::Delete),
            KeyCode::Char('u') => doc.toggle(InlineKind::Insert),
            KeyCode::Char('0') => doc.set_block(BlockKind::Paragraph),
            // Toggle, not set: ⌥1 on a line that's already H1 reverts it to a
            // paragraph, matching the feel of the bold/italic/code toggles.
            KeyCode::Char(d @ '1'..='6') => doc.toggle_heading(d.to_digit(10).unwrap()),
            // Headings stop at 6, so the numeric family keeps going: ⌥7/⌥8 are
            // the other pair that reads as one three-state control (numbered /
            // bulleted / neither), ⌥9 is quote.
            KeyCode::Char('7') => toggle_list(doc, true),
            KeyCode::Char('8') => toggle_list(doc, false),
            KeyCode::Char('9') => doc.toggle_blockquote(),
            KeyCode::Char('k') => open_link_prompt(doc, app),
            KeyCode::Char('s') => open_save_as_prompt(doc, app),
            KeyCode::Char('n') => {
                if doc.dirty {
                    app.dirty_prompt = Some(DirtyPrompt { action: DirtyAction::New, selected: 0 });
                } else {
                    replace_with_blank(doc);
                }
            }
            _ => {}
        }
        return Flow::Continue;
    }

    match key.code {
        KeyCode::Char(c) => doc.insert(&c.to_string()),
        KeyCode::Enter => doc.newline(),
        // In a table, Tab walks the cells (Shift+Tab back) — that precedence
        // is unchanged from before indent/outdent existed. Only once the
        // caret isn't in a table does Tab/Shift+Tab fall through to indent,
        // matching how a Tab in any list/outline editor behaves outside a
        // table and reserving the table's own Tab convention where it applies.
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
    Flow::Continue
}

/// The page step: the body's visible rows minus one for overlap (at least one).
fn page_rows(doc: &Doc) -> usize {
    (doc.body_height as usize).saturating_sub(1).max(1)
}

fn handle_mouse(doc: &mut Doc, m: MouseEvent, app: &mut App) {
    // The menu owns the mouse while it's open: a click on one of its rows runs
    // that row's action, a click anywhere else just dismisses it — either way
    // the click doesn't also fall through to the document underneath (a menu
    // click landing on, say, a paste shouldn't also re-place the caret at the
    // menu's screen position).
    if let Some(menu) = &app.context_menu {
        if let MouseEventKind::Down(_) = m.kind {
            if let Some(i) = menu_item_at(menu, m.row, m.column) {
                let action = MENU_ITEMS[i].1;
                app.context_menu = None;
                action(doc);
            } else {
                app.context_menu = None;
            }
        }
        return;
    }

    let (bx, by) = doc.body_origin;
    let within = m.row >= by
        && (m.row as usize) < by as usize + doc.body_height as usize
        && m.column >= bx;

    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = (m.column - bx) as usize;
            let count = click_count(app, m.row, m.column);
            let shift = m.modifiers.contains(KeyModifiers::SHIFT);

            // Single click places the caret (extending the selection if shift
            // is held, same as a shift-click in any other editor); double
            // selects the word under it; triple selects the block it's in.
            // All three start from the same `click` hit-test so the row/col →
            // offset mapping (source bytes vs. the WYSIWYG glyph grid) only
            // lives in one place.
            //
            // The block, not the source line: a paragraph broken over several
            // lines is one paragraph, and a triple click that stopped at the
            // newline inside it would be selecting a detail of the markup the
            // rich-text view exists to hide. Same call the GUI makes.
            doc.click(row, col, shift);
            match count {
                2 => doc.select_word_at(doc.caret),
                n if n >= 3 => doc.select_block_at(doc.caret),
                _ => {}
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if within => {
            let row = doc.scroll + (m.row - by) as usize;
            let col = (m.column - bx) as usize;
            doc.click(row, col, true); // extend the selection
        }
        // Dragging past the top or bottom edge of the body scrolls to keep
        // revealing more document, the way a drag-select stalling at the
        // viewport edge never does in any other editor. `within`'s column
        // check still applies (dragging off the body sideways isn't this),
        // but its row check is exactly what these two exist to fall outside
        // of, so each re-derives "past the top"/"past the bottom" instead of
        // reusing `within`. `doc.scroll` isn't clamped to the document's
        // length here — `Doc::follow_caret` does that every frame, the same
        // as the wheel handlers below already rely on.
        MouseEventKind::Drag(MouseButton::Left) if m.column >= bx && m.row < by => {
            doc.scroll = doc.scroll.saturating_sub(1);
            let col = (m.column - bx) as usize;
            doc.click(doc.scroll, col, true);
        }
        MouseEventKind::Drag(MouseButton::Left)
            if m.column >= bx && (m.row as usize) >= by as usize + doc.body_height as usize =>
        {
            doc.scroll = doc.scroll.saturating_add(1);
            let col = (m.column - bx) as usize;
            let row = doc.scroll + doc.body_height.saturating_sub(1) as usize;
            doc.click(row, col, true);
        }
        MouseEventKind::Down(MouseButton::Right) if within => {
            // A right-click on top of an existing selection should offer to
            // act on *it* (Cut/Copy), not collapse it to a fresh caret. There's
            // no public way to test "is this screen cell inside the selection"
            // without moving the caret (that mapping is private to `Doc`), so
            // this approximates the precise hit-test the GUI does with the
            // coarser "is any selection active at all" — good enough since a
            // right-click while nothing is selected has no selection to lose.
            if doc.selection().is_none() {
                let row = doc.scroll + (m.row - by) as usize;
                let col = (m.column - bx) as usize;
                doc.click(row, col, false);
            }
            app.context_menu = Some(ContextMenu {
                anchor: (m.column, m.row),
                selected: 0,
                rect: None,
            });
        }
        MouseEventKind::ScrollDown => doc.scroll = doc.scroll.saturating_add(1),
        MouseEventKind::ScrollUp => doc.scroll = doc.scroll.saturating_sub(1),
        _ => {}
    }
}

/// Hit-test a mouse-down against the last-painted context menu rect, returning
/// the row index under it (if any). Mirrors the `doc.body_origin`/
/// `body_height` dance the document body itself uses for the same purpose.
fn menu_item_at(menu: &ContextMenu, row: u16, col: u16) -> Option<usize> {
    let rect = menu.rect?;
    if row >= rect.y && row < rect.y + rect.height && col >= rect.x && col < rect.x + rect.width {
        Some((row - rect.y) as usize)
    } else {
        None
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

/// ⌥7/⌥8: toggle an ordered/bulleted list, then check whether that just
/// nested rather than un-listed. `toggle_list` un-wraps a container only when
/// the edited range covers every block it holds; a bare caret's range is just
/// its own block, so pressing the same list's key a second time inside a
/// multi-item list nests instead of undoing — a real, if surprising, engine
/// rule (see `Doc::toggle_list`), not a bug this frontend can paper over.
/// What it *can* do is stop the nest from reading as "nothing happened": the
/// breadcrumb's count of `kind` ancestors around the caret goes up, not down
/// to zero, exactly when that's what occurred, so that's the signal a status
/// line hangs off.
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

/// ⌥k: open the link prompt, prefilled with the destination of the link the
/// caret already stands in (if any), so re-pointing a link means editing its
/// URL rather than retyping it. A caret outside any link gets an empty box,
/// same as before. Confirming still re-points the link the caret is in, same
/// as `Doc::insert_link` always has.
fn open_link_prompt(doc: &mut Doc, app: &mut App) {
    let initial = doc.link_destination_at_caret().unwrap_or_default();
    app.text_prompt = Some(TextPrompt::new("Link destination", initial, |doc, dest| {
        doc.insert_link(dest);
    }));
}

/// ⌥s and the `dirty_prompt`/conflict flows' Save-As detour: prompt for a
/// destination, prefilled with the document's current path (empty for an
/// untitled one, which just leaves the box empty — there's nothing better to
/// suggest), then move the document there on confirm.
fn open_save_as_prompt(doc: &mut Doc, app: &mut App) {
    let initial = doc.path.to_string_lossy().into_owned();
    app.text_prompt = Some(TextPrompt::new("Save as", initial, |doc, path| {
        doc.save_as(PathBuf::from(path));
    }));
}

/// ⌥n on a clean document, and Discard's answer to a `DirtyAction::New`:
/// swap in a fresh, empty document. `Doc::blank` can only fail on a twig
/// parse of `""`, which isn't a realistic failure, but there's no `Result`
/// for this call site to hand the error to, so it's reported as a status
/// instead of unwrapped into a panic over a user who did nothing wrong.
fn replace_with_blank(doc: &mut Doc) {
    match Doc::blank() {
        Ok(fresh) => *doc = fresh,
        Err(e) => doc.status = Some(format!("new document failed: {e}")),
    }
}

/// Try to save, routing through whichever dialog the situation calls for
/// instead of the two ways a bare `doc.save()` can go wrong silently: an
/// untitled document has no path to write (Save As instead), and a document
/// whose file changed on disk since leaf last touched it would otherwise
/// clobber that change (the overwrite/reload conflict prompt instead).
///
/// `then` is what should happen once the document comes out clean: `None`
/// for a plain ^S, `Some(action)` when Save was chosen to guard a Quit or a
/// New. It's stashed on `app.pending_action` for whichever dialog opens to
/// hand back to `resolve_pending` when it resolves, and resolved immediately
/// when neither dialog is needed.
fn attempt_save(doc: &mut Doc, app: &mut App, then: Option<DirtyAction>) -> Flow {
    if doc.is_untitled() {
        app.pending_action = then;
        open_save_as_prompt(doc, app);
        return Flow::Continue;
    }
    // Only worth the filesystem round-trip `disk_state` costs when there's
    // something of the user's on the line: a document with no unsaved edits
    // has nothing a silent overwrite could lose, so a clean ^S doesn't pay for
    // a read+hash it doesn't need.
    if doc.dirty && doc.disk_state() == DiskState::Changed {
        app.pending_action = then;
        app.conflict = Some(ConflictPrompt { selected: 2 }); // default to Cancel
        return Flow::Continue;
    }
    doc.save();
    app.pending_action = then;
    resolve_pending(doc, app)
}

/// What a save was waiting to do, now that it's had its chance: quit if the
/// save actually landed (`!doc.dirty`), swap in the blank document for New,
/// or — if the write failed — nothing, leaving the failure's status message
/// up instead of pretending the action happened anyway.
fn resolve_pending(doc: &mut Doc, app: &mut App) -> Flow {
    match app.pending_action.take() {
        None => Flow::Continue,
        Some(_) if doc.dirty => Flow::Continue,
        Some(DirtyAction::Quit) => Flow::Quit,
        Some(DirtyAction::New) => {
            replace_with_blank(doc);
            Flow::Continue
        }
    }
}

/// Run the choice made on a `dirty_prompt`: Save (0) hands off to
/// `attempt_save`, Discard (1) runs the guarded action immediately without
/// saving, Cancel (2, or anything else) just closes the prompt. Consumes the
/// prompt either way — Save's continuation past a Save-As or conflict dialog
/// lives on `app.pending_action`, not here.
fn resolve_dirty_prompt(doc: &mut Doc, app: &mut App, choice: usize) -> Flow {
    let action = app.dirty_prompt.take().unwrap().action;
    match choice {
        0 => attempt_save(doc, app, Some(action)),
        1 => match action {
            DirtyAction::Quit => Flow::Quit,
            DirtyAction::New => {
                replace_with_blank(doc);
                Flow::Continue
            }
        },
        _ => Flow::Continue,
    }
}

/// Run the choice made on a `conflict` prompt: Overwrite (0) writes over the
/// external change and lets `resolve_pending` continue whatever was waiting
/// on the save; Reload (1) takes the disk's version instead and drops the
/// pending action — the user asked to catch up with the other write, not to
/// blow past it; Cancel (2, or anything else) leaves the document, and the
/// pending action, untouched, so not saving is always the safe choice.
fn resolve_conflict(doc: &mut Doc, app: &mut App, choice: usize) -> Flow {
    app.conflict = None;
    match choice {
        0 => {
            doc.save();
            resolve_pending(doc, app)
        }
        1 => {
            doc.reload();
            app.pending_action = None;
            Flow::Continue
        }
        _ => {
            app.pending_action = None;
            Flow::Continue
        }
    }
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
            doc.paste(&text);
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
    use leaf_core::View;

    /// A `Doc` over `body`, laid out with the body occupying the whole screen
    /// below a one-row header — the geometry `handle_mouse` hit-tests against.
    fn doc_with(name: &str, body: &str) -> Doc {
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_tui_test_{name}.md"));
        std::fs::write(&p, body).unwrap();
        let mut doc = Doc::open(p).unwrap();
        doc.build_visual(80);
        doc.body_origin = (0, 1);
        doc.body_height = 10;
        doc
    }

    fn left_down(row: u16, col: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn shift_left_down(row: u16, col: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::SHIFT,
        }
    }

    fn right_down(row: u16, col: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn triple_click_selects_the_paragraph_not_the_source_line() {
        // The TUI used to select the source *line* under the click, walking out
        // to the nearest newline. A paragraph broken over two lines is one
        // paragraph, and that newline is markup the WYSIWYG view hides — so the
        // selection stopped in the middle of what it looked like it had grabbed.
        let mut doc = doc_with("triple", "one two\nthree four\n\nnext\n");
        let mut app = App::default();
        for _ in 0..3 {
            handle_mouse(&mut doc, left_down(1, 1), &mut app);
        }
        assert_eq!(doc.selected_text(), Some("one two\nthree four"));
    }

    #[test]
    fn double_click_still_takes_only_the_word() {
        let mut doc = doc_with("double", "one two\nthree four\n\nnext\n");
        let mut app = App::default();
        for _ in 0..2 {
            handle_mouse(&mut doc, left_down(1, 1), &mut app);
        }
        assert_eq!(doc.selected_text(), Some("one"));
    }

    #[test]
    fn shift_click_extends_the_selection_from_the_first_click() {
        let mut doc = doc_with("shift", "one two three\n");
        let mut app = App::default();
        handle_mouse(&mut doc, left_down(1, 0), &mut app); // caret before "one"
        handle_mouse(&mut doc, shift_left_down(1, 9), &mut app); // shift-click into "three"
        assert_eq!(doc.selected_text(), Some("one two t"));
    }

    #[test]
    fn right_click_places_the_caret_and_opens_the_menu() {
        let mut doc = doc_with("right_place", "one two three\n");
        let mut app = App::default();
        handle_mouse(&mut doc, right_down(1, 4), &mut app);
        assert_eq!(doc.caret, 4);
        assert!(app.context_menu.is_some());
    }

    #[test]
    fn right_click_on_a_selection_leaves_it_intact() {
        // Right-clicking inside a selection should offer to act on it (Cut/
        // Copy), not collapse it to a fresh caret the way a left click would.
        let mut doc = doc_with("right_sel", "one two three\n");
        let mut app = App::default();
        for _ in 0..2 {
            handle_mouse(&mut doc, left_down(1, 5), &mut app); // double-click selects "two"
        }
        let before = doc.selected_text().map(str::to_string);
        assert_eq!(before.as_deref(), Some("two"));
        handle_mouse(&mut doc, right_down(1, 5), &mut app);
        assert_eq!(doc.selected_text().map(str::to_string), before);
    }

    #[test]
    fn context_menu_esc_dismisses_without_acting() {
        let mut doc = doc_with("menu_esc", "one two three\n");
        let mut app = App::default();
        handle_mouse(&mut doc, right_down(1, 4), &mut app);
        assert!(app.context_menu.is_some());
        handle_key(&mut doc, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);
        assert!(app.context_menu.is_none());
        assert_eq!(doc.selection(), None);
    }

    #[test]
    fn context_menu_arrows_and_enter_run_the_highlighted_action() {
        let mut doc = doc_with("menu_nav", "one two three\n");
        let mut app = App::default();
        handle_mouse(&mut doc, right_down(1, 4), &mut app);
        // Cut, Copy, Paste, Select All: three Downs from Cut lands on Select All.
        for _ in 0..3 {
            handle_key(&mut doc, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut app);
        }
        handle_key(&mut doc, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert!(app.context_menu.is_none());
        assert_eq!(doc.selected_text(), Some("one two three\n"));
    }

    #[test]
    fn menu_click_on_an_item_runs_it_and_a_click_elsewhere_just_dismisses() {
        let mut doc = doc_with("menu_click", "one two three\n");
        let mut app = App::default();
        handle_mouse(&mut doc, right_down(1, 4), &mut app);
        // The menu hasn't been drawn (no `ui::render` in this test), so there's
        // no painted rect to click on; a click anywhere just dismisses it.
        assert!(app.context_menu.as_ref().unwrap().rect.is_none());
        handle_mouse(&mut doc, left_down(5, 5), &mut app);
        assert!(app.context_menu.is_none());
    }

    #[test]
    fn alt_1_toggles_a_heading_back_to_a_paragraph_and_forth_again() {
        let mut doc = doc_with("heading_toggle", "# Title\n\nbody text\n");
        let mut app = App::default();
        doc.caret = 3; // inside "Title"
        let alt_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);

        handle_key(&mut doc, alt_1, &mut app);
        assert_eq!(&doc.source[..7], "Title\n\n", "first ⌥1 should strip the heading marker");

        handle_key(&mut doc, alt_1, &mut app);
        assert!(doc.source.starts_with("# Title"), "second ⌥1 should re-apply H1");
    }

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

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Like `doc_with`, but a Djot (`.dj`) document — twig's strikethrough
    /// (`Delete`) and underline (`Insert`) marks aren't representable in
    /// Markdown (`toggle` reports "unsupported format" there), so the tests
    /// that exercise ⌥d/⌥u need a format that actually has syntax for them.
    fn doc_with_dj(name: &str, body: &str) -> Doc {
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_tui_test_{name}.dj"));
        std::fs::write(&p, body).unwrap();
        let mut doc = Doc::open(p).unwrap();
        doc.build_visual(80);
        doc.body_origin = (0, 1);
        doc.body_height = 10;
        doc
    }

    fn drag(row: u16, col: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn alt_k_opens_the_link_prompt_empty() {
        let mut doc = doc_with("link_open", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        let prompt = app.text_prompt.as_ref().expect("⌥k should open the prompt");
        assert_eq!(prompt.label, "Link destination");
        assert_eq!(prompt.value, "");
    }

    #[test]
    fn alt_k_prefills_the_existing_link_s_destination() {
        let mut doc = doc_with("link_prefill", "see [t](https://x.dev) ok\n");
        doc.caret = 5; // inside the link's text
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        let prompt = app.text_prompt.as_ref().expect("⌥k should open the prompt");
        assert_eq!(prompt.value, "https://x.dev");
    }

    #[test]
    fn link_prompt_enter_links_the_selection_to_the_typed_destination() {
        let mut doc = doc_with("link_confirm", "hello\n");
        doc.anchor = Some(0);
        doc.caret = 5; // "hello" selected
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        for c in "https://example.com".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        handle_key(&mut doc, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert!(app.text_prompt.is_none(), "Enter should close the prompt");
        assert_eq!(doc.source, "[hello](https://example.com)\n");
    }

    #[test]
    fn link_prompt_esc_cancels_without_touching_the_document() {
        let mut doc = doc_with("link_cancel", "hello\n");
        doc.anchor = Some(0);
        doc.caret = 5;
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        for c in "http://x".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        handle_key(&mut doc, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app);
        assert!(app.text_prompt.is_none());
        assert_eq!(doc.source, "hello\n");
    }

    #[test]
    fn link_prompt_backspace_deletes_the_last_character_typed() {
        let mut doc = doc_with("link_backspace", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        for c in "abc".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        handle_key(&mut doc, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), &mut app);
        assert_eq!(app.text_prompt.as_ref().unwrap().value, "ab");
    }

    #[test]
    fn text_prompt_owns_the_keyboard_document_keys_dont_leak_through() {
        // ^A would select-all and ⌥b would toggle bold on the document if
        // either reached it; while the prompt is open both must land as
        // ordinary characters typed into the box (or nothing, for ^A's 'a'
        // colliding with a letter — the point is *not* the document op) —
        // never the document command.
        let mut doc = doc_with("prompt_isolation", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        handle_key(&mut doc, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL), &mut app);
        assert_eq!(doc.selection(), None, "^A must not have reached select_all");
        handle_key(&mut doc, alt('b'), &mut app);
        assert_eq!(doc.source, "hello\n", "⌥b must not have reached the document");
        assert!(app.text_prompt.is_some(), "the prompt should still be open");
        assert_eq!(app.text_prompt.unwrap().value, "ab");
    }

    #[test]
    fn alt_8_toggles_a_bulleted_list_at_the_caret() {
        let mut doc = doc_with("list8", "item\n");
        let mut app = App::default();
        doc.caret = 0;
        handle_key(&mut doc, alt('8'), &mut app);
        assert_eq!(doc.source, "- item\n");
    }

    #[test]
    fn alt_7_toggles_a_numbered_list_at_the_caret() {
        let mut doc = doc_with("list7", "item\n");
        let mut app = App::default();
        doc.caret = 0;
        handle_key(&mut doc, alt('7'), &mut app);
        assert_eq!(doc.source, "1. item\n");
    }

    #[test]
    fn alt_9_toggles_a_blockquote_at_the_caret() {
        let mut doc = doc_with("quote9", "item\n");
        let mut app = App::default();
        doc.caret = 0;
        handle_key(&mut doc, alt('9'), &mut app);
        assert_eq!(doc.source, "> item\n");
    }

    #[test]
    fn alt_8_with_a_full_selection_removes_the_list_without_a_nest_message() {
        let mut doc = doc_with("list_unwrap", "- item\n");
        let mut app = App::default();
        doc.anchor = Some(0);
        doc.caret = doc.source.len();
        handle_key(&mut doc, alt('8'), &mut app);
        assert_eq!(doc.source, "item\n");
        assert_eq!(doc.status, None);
    }

    #[test]
    fn alt_8_on_a_bare_caret_in_a_multi_item_list_nests_and_says_so() {
        // The known engine rule from the task: an empty range only ever
        // covers the caret's own block, and a container comes off only when
        // the edited range covers every block it holds — so a second-item
        // caret nests instead of un-listing. This asserts the status line
        // says so rather than leaving the nest looking like a no-op.
        let mut doc = doc_with("list_nest", "- a\n- b\n");
        let mut app = App::default();
        doc.caret = doc.source.find('b').unwrap();
        handle_key(&mut doc, alt('8'), &mut app);
        assert!(doc.source.contains("- - b"), "the second item should have nested: {:?}", doc.source);
        assert!(
            doc.status.as_deref().unwrap_or("").contains("nested"),
            "status should explain the nest: {:?}",
            doc.status
        );
    }

    // ── quit / save / discard ────────────────────────────────────────────────

    #[test]
    fn ctrl_q_on_a_clean_document_quits_immediately() {
        let mut doc = doc_with("quit_clean", "hello\n");
        let mut app = App::default();
        assert!(!doc.dirty);
        assert!(handle_key(&mut doc, ctrl('q'), &mut app) == Flow::Quit);
    }

    #[test]
    fn ctrl_q_on_a_dirty_document_opens_a_save_discard_cancel_prompt() {
        // The old y/n confirmation could only quit *without* saving; this is
        // the three-way choice item 1 replaces it with, defaulted to Save —
        // the choice an accidental Enter should make.
        let mut doc = doc_with("quit_dirty", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        let mut app = App::default();
        assert!(handle_key(&mut doc, ctrl('q'), &mut app) == Flow::Continue);
        let prompt = app.dirty_prompt.as_ref().expect("a dirty ^Q should open the prompt");
        assert!(prompt.action == DirtyAction::Quit);
        assert_eq!(prompt.selected, 0);
    }

    #[test]
    fn dirty_prompt_cancel_leaves_the_document_untouched() {
        let mut doc = doc_with("quit_cancel", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('q'), &mut app);
        assert!(handle_key(&mut doc, plain('c'), &mut app) == Flow::Continue);
        assert!(app.dirty_prompt.is_none());
        assert_eq!(doc.source, "hello world\n");
        assert!(doc.dirty);
    }

    #[test]
    fn dirty_prompt_discard_quits_without_writing_the_file() {
        let mut doc = doc_with("quit_discard", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('q'), &mut app);
        assert!(handle_key(&mut doc, plain('d'), &mut app) == Flow::Quit);
        assert_eq!(std::fs::read_to_string(&doc.path).unwrap(), "hello\n");
    }

    #[test]
    fn dirty_prompt_save_writes_the_file_and_then_quits() {
        let mut doc = doc_with("quit_save", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('q'), &mut app);
        assert!(handle_key(&mut doc, plain('s'), &mut app) == Flow::Quit);
        assert_eq!(std::fs::read_to_string(&doc.path).unwrap(), "hello world\n");
    }

    #[test]
    fn ctrl_s_on_an_untitled_document_routes_to_save_as_instead_of_failing() {
        let mut doc = Doc::blank().unwrap();
        doc.insert("hello");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);
        let prompt = app.text_prompt.as_ref().expect("^S on an untitled doc should open Save As");
        assert_eq!(prompt.label, "Save as");
        assert_eq!(prompt.value, "");
        assert!(doc.is_untitled(), "no path should have been invented");
    }

    #[test]
    fn save_as_confirm_writes_the_file_and_adopts_the_path() {
        let mut doc = Doc::blank().unwrap();
        doc.insert("hello");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);

        let mut p = std::env::temp_dir();
        p.push("leaf_tui_test_saveas_confirm.md");
        let _ = std::fs::remove_file(&p);
        for c in p.to_string_lossy().chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        handle_key(&mut doc, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);

        assert!(app.text_prompt.is_none());
        assert!(!doc.is_untitled());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
    }

    #[test]
    fn quitting_an_untitled_dirty_document_quits_only_after_the_save_as_lands() {
        // The interplay item 1 and item 6 create: Save from the quit prompt on
        // an untitled document can't write anywhere yet, so it has to detour
        // through Save As, and only *that* landing should let the pending quit
        // through — not the keystroke that opened the detour.
        let mut doc = Doc::blank().unwrap();
        doc.insert("hello");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('q'), &mut app);
        assert!(handle_key(&mut doc, plain('s'), &mut app) == Flow::Continue);
        assert!(app.text_prompt.is_some(), "Save should have detoured to Save As");
        assert!(app.dirty_prompt.is_none());

        let mut p = std::env::temp_dir();
        p.push("leaf_tui_test_quit_via_saveas.md");
        let _ = std::fs::remove_file(&p);
        for c in p.to_string_lossy().chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        let flow = handle_key(&mut doc, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut app);
        assert!(flow == Flow::Quit, "the pending quit should fire once the save-as lands");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
    }

    #[test]
    fn escaping_the_save_as_detour_abandons_the_quit_too() {
        let mut doc = Doc::blank().unwrap();
        doc.insert("hello");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('q'), &mut app);
        handle_key(&mut doc, plain('s'), &mut app);
        assert!(app.text_prompt.is_some());
        assert!(handle_key(&mut doc, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut app) == Flow::Continue);
        assert!(app.text_prompt.is_none());
        assert!(app.dirty_prompt.is_none(), "canceling the destination shouldn't resurrect the quit prompt");
        assert!(doc.dirty);
    }

    // ── new document ─────────────────────────────────────────────────────────

    #[test]
    fn alt_n_on_a_clean_document_replaces_it_immediately() {
        let mut doc = doc_with("new_clean", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('n'), &mut app);
        assert!(doc.is_untitled());
        assert_eq!(doc.source, "");
    }

    #[test]
    fn alt_n_on_a_dirty_document_asks_first() {
        let mut doc = doc_with("new_dirty", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        let mut app = App::default();
        handle_key(&mut doc, alt('n'), &mut app);
        let prompt = app.dirty_prompt.as_ref().expect("⌥n on a dirty doc should ask first");
        assert!(prompt.action == DirtyAction::New);
        assert_eq!(doc.source, "hello world\n", "nothing should change before the choice is made");
    }

    #[test]
    fn alt_n_dirty_prompt_discard_replaces_the_document() {
        let mut doc = doc_with("new_discard", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        let mut app = App::default();
        handle_key(&mut doc, alt('n'), &mut app);
        assert!(handle_key(&mut doc, plain('d'), &mut app) == Flow::Continue);
        assert!(doc.is_untitled());
        assert_eq!(doc.source, "");
    }

    // ── external-change conflict ─────────────────────────────────────────────

    #[test]
    fn ctrl_s_stops_for_a_file_changed_on_disk_instead_of_clobbering_it() {
        let mut doc = doc_with("conflict", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        std::fs::write(&doc.path, "someone else's edit\n").unwrap(); // external write
        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);
        let prompt = app.conflict.as_ref().expect("a changed file should stop the save");
        assert_eq!(prompt.selected, 2, "the safe default is Cancel, not Overwrite");
        assert_eq!(std::fs::read_to_string(&doc.path).unwrap(), "someone else's edit\n");
    }

    #[test]
    fn conflict_reload_takes_the_disk_version_and_drops_the_local_edits() {
        let mut doc = doc_with("conflict_reload", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        std::fs::write(&doc.path, "someone else's edit\n").unwrap();
        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);
        assert!(handle_key(&mut doc, plain('r'), &mut app) == Flow::Continue);
        assert!(app.conflict.is_none());
        assert_eq!(doc.source, "someone else's edit\n");
        assert!(!doc.dirty);
    }

    #[test]
    fn conflict_overwrite_writes_over_the_external_change() {
        let mut doc = doc_with("conflict_overwrite", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        std::fs::write(&doc.path, "someone else's edit\n").unwrap();
        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);
        assert!(handle_key(&mut doc, plain('o'), &mut app) == Flow::Continue);
        assert_eq!(std::fs::read_to_string(&doc.path).unwrap(), "hello world\n");
    }

    // ── indent / outdent / kill line ─────────────────────────────────────────

    #[test]
    fn tab_indents_two_spaces_not_the_four_space_code_block_marker() {
        let mut doc = doc_with("indent", "line one\n");
        let mut app = App::default();
        doc.caret = 0;
        handle_key(&mut doc, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut app);
        assert_eq!(doc.source, "  line one\n");
    }

    #[test]
    fn shift_tab_outdents_one_level() {
        let mut doc = doc_with("outdent", "    line one\n");
        let mut app = App::default();
        doc.caret = 0;
        handle_key(&mut doc, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
        assert_eq!(doc.source, "  line one\n");
    }

    #[test]
    fn tab_in_a_table_hops_cells_instead_of_indenting() {
        // Table cell-hop takes precedence over indent — the same Tab that
        // indents everywhere else keeps walking cells inside a table, exactly
        // as it did before indent/outdent existed.
        let mut doc = doc_with("table_tab", "| a | b |\n| - | - |\n| c | d |\n");
        let mut app = App::default();
        doc.caret = doc.source.find('a').unwrap();
        handle_key(&mut doc, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut app);
        assert_eq!(doc.source, "| a | b |\n| - | - |\n| c | d |\n", "a table hop must not indent");
        assert_eq!(doc.caret, doc.source.find('b').unwrap());
    }

    #[test]
    fn ctrl_u_kills_back_to_the_line_start() {
        let mut doc = doc_with("kill_start", "hello world\n");
        let mut app = App::default();
        doc.caret = 5; // just after "hello"
        handle_key(&mut doc, ctrl('u'), &mut app);
        assert_eq!(doc.source, " world\n");
    }

    #[test]
    fn ctrl_k_kills_forward_to_the_line_end() {
        let mut doc = doc_with("kill_end", "hello world\n");
        let mut app = App::default();
        doc.caret = 5; // just after "hello"
        handle_key(&mut doc, ctrl('k'), &mut app);
        assert_eq!(doc.source, "hello\n");
    }

    // ── strikethrough / underline ────────────────────────────────────────────

    #[test]
    fn alt_d_toggles_strikethrough_on_the_selection() {
        let mut doc = doc_with_dj("strike", "hello world\n");
        let mut app = App::default();
        doc.anchor = Some(0);
        doc.caret = 5; // "hello" selected
        handle_key(&mut doc, alt('d'), &mut app);
        assert!(doc.active_inline_marks().contains(InlineKind::Delete), "status: {:?}", doc.status);
    }

    #[test]
    fn alt_u_toggles_underline_on_the_selection() {
        let mut doc = doc_with_dj("underline", "hello world\n");
        let mut app = App::default();
        doc.anchor = Some(0);
        doc.caret = 5; // "hello" selected
        handle_key(&mut doc, alt('u'), &mut app);
        assert!(doc.active_inline_marks().contains(InlineKind::Insert), "status: {:?}", doc.status);
    }

    // ── paste ────────────────────────────────────────────────────────────────

    #[test]
    fn clipboard_paste_uses_doc_paste_not_doc_insert() {
        // `Doc::paste` (unlike `insert`) is always its own undo step, even for
        // one character — the observable difference is that a paste right
        // after typing does *not* coalesce into that typing run's undo.
        let mut doc = doc_with("paste_coalesce", "");
        doc.insert("a"); // a one-character typing run
        set_clipboard_text("b".into()).ok(); // best-effort: skip if no clipboard
        clipboard_paste(&mut doc);
        if doc.status.as_deref() == Some("clipboard unavailable") {
            return; // headless CI/sandbox with no system clipboard
        }
        assert_eq!(doc.source, "ab");
        doc.undo();
        assert_eq!(doc.source, "a", "undo should peel off only the pasted 'b'");
    }

    // ── drag autoscroll ──────────────────────────────────────────────────────

    #[test]
    fn dragging_past_the_bottom_edge_scrolls_down_and_keeps_selecting() {
        // Source view, not WYSIWYG: WYSIWYG joins bare lines with soft breaks
        // into one wrapped paragraph, so "row 10" isn't the tenth line the way
        // it is here — the row/col → offset mapping this is exercising is
        // `handle_mouse`'s scroll bookkeeping, not either view's own mapping.
        let mut doc = doc_with("drag_down", &"line\n".repeat(30));
        doc.view = View::Source;
        let mut app = App::default();
        handle_mouse(&mut doc, left_down(1, 0), &mut app); // caret at the top row
        let before = doc.scroll;
        handle_mouse(&mut doc, drag(11, 0), &mut app); // one row past body_height (10)
        assert!(doc.scroll > before, "dragging past the bottom edge should scroll down");
        assert!(doc.selection().is_some(), "the drag should still be extending a selection");
    }

    #[test]
    fn dragging_past_the_top_edge_scrolls_up() {
        let mut doc = doc_with("drag_up", &"line\n".repeat(30));
        doc.view = View::Source;
        doc.scroll = 5;
        let mut app = App::default();
        handle_mouse(&mut doc, left_down(2, 0), &mut app);
        handle_mouse(&mut doc, drag(0, 0), &mut app); // above body_origin's row (1)
        assert_eq!(doc.scroll, 4);
    }
}
