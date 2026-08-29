//! leaf — a caret-based rich-text TUI editor for documents, built on twig.
//!
//! Sibling to bough: same twig backend, opposite interaction model. bough moves
//! a selection through the AST and edits the tree; leaf gives you a text caret,
//! mouse, and a formatting toolbar, and turns each keystroke into an
//! offset-addressed twig edit that reparses live. You type into a document that
//! stays a valid AST the whole time.

mod commands;
mod palette;
mod ui;

use std::io::stdout;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use leaf_core::{Alignment, DiskState, Doc, InlineKind, LineFlow, MarkupMode, MediaKind};
use leaf_ratatui::{MouseOutcome, Outcome};

use commands::{Command, Ctx};
use palette::Palette;
use ratatui::{
    crossterm::{
        event::{
            self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent, MouseEventKind,
        },
        execute,
    },
    layout::Rect,
};

/// The one-line usage, shared by the no-argument error and `--help` so the two
/// can never drift apart. A path that isn't there yet is named as part of the
/// usage because it's a supported way to start, not a tolerated mistake: leaf
/// opens an empty buffer for it and the first save creates the file.
const USAGE: &str =
    "usage: leaf <file.md|file.dj|file.html|file.xml>   (a missing file is created on save)";

fn main() -> Result<()> {
    let arg = std::env::args_os()
        .nth(1)
        .ok_or_else(|| anyhow!("{USAGE}"))?;

    // `--version` and `--help` are answered before the file is opened or the
    // terminal is entered. Homebrew's formula test is `leaf --version` on a
    // machine with no document to hand, and every argument below this point is
    // treated as a path — so a flag that fell through would be opened as a
    // filename and exit non-zero. Printing the crate version is also what makes
    // that test meaningful: it is what `brew` matches the formula's version
    // against, which is how a mis-tagged release gets caught.
    if let Some(flag) = arg.to_str() {
        match flag {
            "--version" | "-V" => {
                println!("leaf {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => {}
        }
    }

    // `open_or_create`, not `open`: naming a file that doesn't exist is how every
    // other terminal editor is asked to start a new one, and refusing it sent the
    // user off to `touch` a file leaf is about to write anyway. What comes back is
    // a fully named document — ^S writes it, the header shows its name — so the
    // only difference from an opened file is that it's empty. A path leaf can't
    // parse (no extension, or an unknown one) is still an error, so a mistyped
    // flag doesn't silently become a buffer.
    let path = PathBuf::from(arg);
    // Cosmetic only — it decides a status message, nothing about how the file is
    // opened. `open_or_create` makes that decision from its own failed read, so
    // this cheap `stat` racing the open can at worst mislabel a file somebody
    // created in the microseconds between them.
    let existed = path.exists();
    let mut doc = Doc::open_or_create(path)?;
    if !existed {
        // Say it once, in the status line the editor already has: an empty screen
        // under a filename could otherwise be read as "leaf lost my file". It
        // clears on the first keystroke like every other status message.
        doc.status = Some(format!("{} — new file", doc.file_name()));
    }

    let mut terminal = ratatui::init();
    // Mouse capture, and — the other half of "the terminal tells us what the
    // user did rather than making us guess" — bracketed paste. Without it a
    // paste is delivered as a burst of ordinary key presses, which is not just
    // slow: each character is a separate `Doc::insert`, so the paste is a
    // hundred undo steps and a hundred list-continuation/autoformat decisions
    // taken on text nobody typed. With it the run arrives whole, as
    // `Event::Paste`, and goes through the same `Doc::paste` the clipboard
    // verbs use. Both are turned back off on the way out, in the reverse
    // order, so a terminal leaf crashed in isn't left in either mode.
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = run(&mut terminal, &mut doc);
    let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    result
}

/// Host-only state that belongs to neither `Doc` nor the editor widget: the
/// modal dialogs (quit/new confirmation, on-disk conflict), the right-click
/// context menu, and the single-line text prompt. The editing surface's own
/// view state — horizontal scroll, the image cache, click-counting — lives on
/// the widget's [`leaf_ratatui::EditorState`]; this is just the chrome the host
/// wraps around it.
#[derive(Default)]
struct App {
    /// Set by Ctrl+Q or ⌥n meeting a dirty document: a centered overlay offers a
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
    /// Present while the right-click menu is open; consumes keyboard and
    /// mouse input until an item is chosen or it's dismissed.
    context_menu: Option<ContextMenu>,
    /// Present while a single-line input (the link-destination prompt today,
    /// Save As later) is open; consumes the keyboard the same way
    /// `context_menu` does, until Enter confirms or Esc cancels it.
    text_prompt: Option<TextPrompt>,
    /// Present while the command palette is open. Like the menu it consumes the
    /// keyboard and the mouse until a command is chosen or it's dismissed — but
    /// unlike the menu it also has a query line, so it takes printable keys too.
    palette: Option<Palette>,
    /// Whether the key reference is up. A read-only overlay with no state of its
    /// own beyond "showing": any key closes it.
    help: bool,
    /// The editor widget's own view state: horizontal/code scroll, the image
    /// raster cache, and mouse click-counting. Threaded into
    /// `leaf_ratatui::render`/`handle_key`/`handle_mouse` each frame.
    editor: leaf_ratatui::EditorState,
}

/// One row of the context menu. `Action` runs a [`Command`] and closes the menu;
/// `Submenu` opens a flyout of further rows; `Header` is a dim, unselectable
/// section label — the divider between the block and inline styles.
///
/// An `Action` carries no label of its own: the command already knows what it's
/// called, and a second copy of the word here is a second place for it to drift.
/// `ui::render_context_menu` reads labels, keys, checkmarks, and availability
/// off these same values, so what's drawn and what's wired can't disagree.
#[derive(Clone, Copy)]
pub enum MenuEntry {
    Action(Command),
    Submenu(&'static str, &'static [MenuEntry]),
    Header(&'static str),
}

impl MenuEntry {
    pub fn label(self) -> &'static str {
        match self {
            MenuEntry::Action(c) => c.label(),
            MenuEntry::Submenu(l, _) | MenuEntry::Header(l) => l,
        }
    }

    /// The key that runs this row, for the right-hand column. Submenus and
    /// headers have none.
    pub fn hint(self) -> &'static str {
        match self {
            MenuEntry::Action(c) => c.hint(),
            _ => "",
        }
    }

    /// Whether this row can be highlighted and run in the document as it stands.
    /// A `Header` never can. A `Submenu` can exactly when something inside it
    /// can — so the Table flyout dims itself away when the caret isn't in a
    /// table, without the menu tree having to state that condition twice.
    fn selectable(self, ctx: &Ctx) -> bool {
        match self {
            MenuEntry::Header(_) => false,
            MenuEntry::Action(c) => c.enabled(ctx),
            MenuEntry::Submenu(_, items) => items.iter().any(|e| e.selectable(ctx)),
        }
    }
}

/// The root right-click menu. The four clipboard verbs, then a flyout per family
/// — the same families the palette and the key reference group by.
///
/// `Format` stays at index 4: the block/inline flyout is the one people reach
/// for constantly, and the three added beside it are appended rather than
/// interleaved so the muscle memory for it survives.
pub const ROOT_MENU: &[MenuEntry] = &[
    MenuEntry::Action(Command::Cut),
    MenuEntry::Action(Command::Copy),
    MenuEntry::Action(Command::Paste),
    MenuEntry::Action(Command::SelectAll),
    MenuEntry::Submenu("Format", FORMAT_MENU),
    MenuEntry::Submenu("Insert", INSERT_MENU),
    MenuEntry::Submenu("Table", TABLE_MENU),
    MenuEntry::Submenu("View", VIEW_MENU),
    MenuEntry::Action(Command::Follow),
];

/// Every styling command the keyboard exposes, gathered into one flyout and
/// split into a block section (what the whole paragraph becomes) and an inline
/// section (marks on the selection).
pub const FORMAT_MENU: &[MenuEntry] = &[
    MenuEntry::Header("Block"),
    MenuEntry::Action(Command::Paragraph),
    MenuEntry::Action(Command::Heading(1)),
    MenuEntry::Action(Command::Heading(2)),
    MenuEntry::Action(Command::Heading(3)),
    MenuEntry::Action(Command::BulletList),
    MenuEntry::Action(Command::NumberedList),
    MenuEntry::Action(Command::Quote),
    MenuEntry::Action(Command::TaskItem),
    MenuEntry::Action(Command::TaskChecked),
    MenuEntry::Header("Inline"),
    MenuEntry::Action(Command::Inline(InlineKind::Strong)),
    MenuEntry::Action(Command::Inline(InlineKind::Emph)),
    MenuEntry::Action(Command::Inline(InlineKind::Verbatim)),
    MenuEntry::Action(Command::Inline(InlineKind::Mark)),
    MenuEntry::Action(Command::Inline(InlineKind::Delete)),
    MenuEntry::Action(Command::Inline(InlineKind::Insert)),
];

/// What can be put *into* the document that isn't a restyling of what's already
/// there. The three media kinds sit together because they are one control with
/// three destinations behind it, which is how core gates them too.
pub const INSERT_MENU: &[MenuEntry] = &[
    MenuEntry::Action(Command::Link),
    MenuEntry::Action(Command::Image),
    MenuEntry::Action(Command::Video),
    MenuEntry::Action(Command::Audio),
    MenuEntry::Action(Command::Footnote),
    MenuEntry::Action(Command::ThematicBreak),
    MenuEntry::Action(Command::CodeLanguage),
];

/// The grid controls, in the order a spreadsheet puts them: add, remove, move,
/// align. Every row here needs a caret in a table, so the whole flyout dims
/// together when there isn't one — which is the honest shape, since a table menu
/// off a table has nothing partial to offer.
pub const TABLE_MENU: &[MenuEntry] = &[
    MenuEntry::Header("Rows"),
    MenuEntry::Action(Command::RowAbove),
    MenuEntry::Action(Command::RowBelow),
    MenuEntry::Action(Command::DeleteRow),
    MenuEntry::Action(Command::MoveRowUp),
    MenuEntry::Action(Command::MoveRowDown),
    MenuEntry::Header("Columns"),
    MenuEntry::Action(Command::ColumnLeft),
    MenuEntry::Action(Command::ColumnRight),
    MenuEntry::Action(Command::DeleteColumn),
    MenuEntry::Action(Command::MoveColumnLeft),
    MenuEntry::Action(Command::MoveColumnRight),
    MenuEntry::Header("Alignment"),
    MenuEntry::Action(Command::Align(Alignment::Left)),
    MenuEntry::Action(Command::Align(Alignment::Center)),
    MenuEntry::Action(Command::Align(Alignment::Right)),
    MenuEntry::Action(Command::Align(Alignment::Default)),
];

/// How the document is *shown* rather than what it says — the one flyout whose
/// rows are radio groups, so every row here carries a live checkmark naming the
/// setting in force.
pub const VIEW_MENU: &[MenuEntry] = &[
    MenuEntry::Action(Command::ToggleView),
    MenuEntry::Header("Markup"),
    MenuEntry::Action(Command::CycleMarkup),
    MenuEntry::Action(Command::Markup(MarkupMode::None)),
    MenuEntry::Action(Command::Markup(MarkupMode::Shortcuts)),
    MenuEntry::Action(Command::Markup(MarkupMode::Full)),
    MenuEntry::Header("Line flow"),
    MenuEntry::Action(Command::ToggleFlow),
    MenuEntry::Action(Command::Flow(LineFlow::Fold)),
    MenuEntry::Action(Command::Flow(LineFlow::Preserve)),
];

/// The right-click menu, as a stack of open levels: the root first, then any
/// submenu drilled into. The last level owns the keyboard; Esc/Left pops it, and
/// a click or hover on a `Submenu` row pushes the next. It's the one piece of
/// host chrome with a real navigation state of its own.
pub struct ContextMenu {
    /// Screen cell the right-click landed on; the root level is anchored here
    /// (nudged back on screen if it wouldn't fit) and each submenu flies out
    /// from its parent row.
    anchor: (u16, u16),
    /// The open levels, root first. Never empty while the menu is up.
    levels: Vec<MenuLevel>,
}

pub struct MenuLevel {
    items: &'static [MenuEntry],
    /// The highlighted row — moved by the arrow keys and by mouse hover, always
    /// left on a selectable (non-`Header`) row.
    selected: usize,
    /// The rect `ui::render_context_menu` last painted this level at, stashed for
    /// hit-testing the same way `doc.body_origin` is.
    rect: Option<Rect>,
}

impl MenuLevel {
    fn new(items: &'static [MenuEntry], ctx: &Ctx) -> Self {
        let selected = items.iter().position(|e| e.selectable(ctx)).unwrap_or(0);
        MenuLevel {
            items,
            selected,
            rect: None,
        }
    }

    /// Move the highlight `delta` rows, skipping headers and rows this document
    /// can't run, and wrapping at the ends. A no-op for a level with nothing
    /// selectable — which a real menu can now be (the Table flyout off a table),
    /// so the walk has to stay total rather than merely happening to be.
    fn step(&mut self, delta: isize, ctx: &Ctx) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.selected as isize;
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if self.items[i as usize].selectable(ctx) {
                self.selected = i as usize;
                return;
            }
        }
    }
}

impl ContextMenu {
    fn new(anchor: (u16, u16), ctx: &Ctx) -> Self {
        ContextMenu {
            anchor,
            levels: vec![MenuLevel::new(ROOT_MENU, ctx)],
        }
    }

    /// The frontmost (deepest) level — the one the keyboard drives.
    fn active_level(&self) -> usize {
        self.levels.len() - 1
    }

    /// Open `items` as the submenu of level `parent`, replacing any deeper level
    /// already showing (hovering a different submenu row swaps the flyout). A
    /// no-op if this exact submenu is already open, so hovering its parent row
    /// doesn't keep resetting the child's own highlight.
    fn open_submenu(&mut self, parent: usize, items: &'static [MenuEntry], ctx: &Ctx) {
        if self
            .levels
            .get(parent + 1)
            .is_some_and(|l| l.items.as_ptr() == items.as_ptr())
        {
            return;
        }
        self.levels.truncate(parent + 1);
        self.levels.push(MenuLevel::new(items, ctx));
    }

    /// The `(level, row)` under a screen cell, deepest level first so a submenu
    /// wins over the parent it flies out over.
    fn hit(&self, row: u16, col: u16) -> Option<(usize, usize)> {
        for (i, level) in self.levels.iter().enumerate().rev() {
            if let Some(rect) = level.rect
                && row >= rect.y
                && row < rect.y + rect.height
                && col >= rect.x
                && col < rect.x + rect.width
            {
                let idx = (row - rect.y) as usize;
                if idx < level.items.len() {
                    return Some((i, idx));
                }
            }
        }
        None
    }
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
    fn new(
        label: &'static str,
        initial: impl Into<String>,
        on_confirm: fn(&mut Doc, &str),
    ) -> Self {
        let value = initial.into();
        let cursor = value.len();
        TextPrompt {
            label,
            value,
            cursor,
            on_confirm,
        }
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
/// floated as a centered modal (`ui::render_choice_overlay`); unlike the menu
/// there's no click to anchor it to, so it centers on the screen instead.
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
    // Probe the terminal for its graphics protocol now that `ratatui::init` has
    // put it in raw mode — the query reads escape-sequence replies. A terminal
    // that can't answer keeps the half-blocks fallback.
    app.editor.query_graphics();
    // …and ask it whether it's light or dark, for the same reason and in the
    // same window: the palette behind code has to be tinted toward the user's
    // actual background, not toward an assumed black one. `LEAF_THEME=light|dark`
    // overrides the answer for a terminal that won't give a straight one.
    app.editor.query_color_scheme();
    loop {
        terminal.draw(|f| ui::render(f, doc, &mut app))?;

        match event::read()? {
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    && handle_key(doc, key, &mut app) == Flow::Quit =>
            {
                return Ok(());
            }
            // Mouse motion (with no button down) drives the context menu's hover
            // highlight; `EnableMouseCapture` already turns on any-motion
            // reporting, so these `Moved` events arrive without extra setup. The
            // editing surface ignores them, so they cost only a redraw.
            Event::Mouse(m) => handle_mouse(doc, m, &mut app),
            // A terminal paste, arriving whole because `main` turned bracketed
            // paste on.
            Event::Paste(text) => handle_paste(doc, &text, &mut app),
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
    // do: arrows move the highlight (skipping section headers), Right/Enter open
    // a submenu or run the highlighted row, Left/Esc back out one level (or
    // close at the root), and any other key closes it without acting.
    if app.context_menu.is_some() {
        let ctx = Ctx::read(doc);
        let menu = app.context_menu.as_mut().unwrap();
        let lvl = menu.active_level();
        let entry = menu.levels[lvl].items[menu.levels[lvl].selected];
        match key.code {
            KeyCode::Up => menu.levels[lvl].step(-1, &ctx),
            KeyCode::Down => menu.levels[lvl].step(1, &ctx),
            KeyCode::Right => {
                if let MenuEntry::Submenu(_, items) = entry {
                    menu.open_submenu(lvl, items, &ctx);
                }
            }
            KeyCode::Enter => match entry {
                MenuEntry::Action(cmd) => {
                    app.context_menu = None;
                    let outcome = cmd.run(doc);
                    return apply_outcome(doc, app, outcome);
                }
                MenuEntry::Submenu(_, items) => menu.open_submenu(lvl, items, &ctx),
                MenuEntry::Header(_) => {}
            },
            KeyCode::Left | KeyCode::Esc => {
                if lvl > 0 {
                    menu.levels.pop();
                } else {
                    app.context_menu = None;
                }
            }
            _ => app.context_menu = None,
        }
        return Flow::Continue;
    }

    // The key reference is a read-only card: it answers nothing, so any key at
    // all puts it away rather than making the reader hunt for the one that does.
    if app.help {
        app.help = false;
        return Flow::Continue;
    }

    // The palette owns the keyboard the way the text prompt does — it *is* a
    // text prompt, with a list under it — so nothing below may leak through to
    // the document while it's up.
    if app.palette.is_some() {
        return handle_palette_key(doc, key, app);
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

    // No overlay is capturing input, so the editing surface gets the key. It
    // performs any document edit itself and returns what the *host* must do —
    // quit, save, clipboard, or open one of its own dialogs.
    let outcome = leaf_ratatui::handle_key(doc, key, &mut app.editor);
    apply_outcome(doc, app, outcome)
}

/// Carry out an [`Outcome`] — the one place the host's own verbs live.
///
/// Both input paths end here: a key press, whose outcome the editing surface
/// returns, and a command chosen from the palette or the context menu, which
/// returns the *same* type from [`Command::run`]. That convergence is the point.
/// Before it, "⌥k opens the link prompt" was written in the widget and "the Link
/// menu row opens the link prompt" would have had to be written again here; now
/// a command's host-side behaviour is stated once and both doors reach it.
fn apply_outcome(doc: &mut Doc, app: &mut App, outcome: Outcome) -> Flow {
    match outcome {
        Outcome::Continue => Flow::Continue,
        // Ctrl+Q: quit, guarding an unsaved document behind the Save/Discard/
        // Cancel prompt the way it always did.
        Outcome::Quit => {
            if doc.dirty {
                app.dirty_prompt = Some(DirtyPrompt {
                    action: DirtyAction::Quit,
                    selected: 0,
                });
                Flow::Continue
            } else {
                Flow::Quit
            }
        }
        // Ctrl+S: save, routing through the Save-As / conflict dialogs as needed.
        Outcome::Save => {
            attempt_save(doc, app, None);
            Flow::Continue
        }
        // ⌥S: name a destination and move the document there.
        Outcome::SaveAs => {
            open_save_as_prompt(doc, app);
            Flow::Continue
        }
        // ⌥N: swap in a blank document, guarding unsaved changes first.
        Outcome::New => {
            if doc.dirty {
                app.dirty_prompt = Some(DirtyPrompt {
                    action: DirtyAction::New,
                    selected: 0,
                });
            } else {
                replace_with_blank(doc);
            }
            Flow::Continue
        }
        // Clipboard (^C/^X/^V and ⌥V) — the host owns the system pasteboard.
        Outcome::Copy => {
            clipboard_copy(doc);
            Flow::Continue
        }
        Outcome::Cut => {
            clipboard_cut(doc);
            Flow::Continue
        }
        Outcome::Paste => {
            clipboard_paste(doc);
            Flow::Continue
        }
        Outcome::PastePlain => {
            clipboard_paste_plain(doc);
            Flow::Continue
        }
        // ⌥K / ⌥L / ⌥E and their menu rows: open a single-line prompt the host owns.
        Outcome::LinkPrompt => {
            open_link_prompt(doc, app);
            Flow::Continue
        }
        Outcome::LanguagePrompt => {
            open_language_prompt(doc, app);
            Flow::Continue
        }
        Outcome::ImagePrompt => {
            open_media_prompt(app, MediaKind::Image);
            Flow::Continue
        }
        Outcome::VideoPrompt => {
            open_media_prompt(app, MediaKind::Video);
            Flow::Continue
        }
        Outcome::AudioPrompt => {
            open_media_prompt(app, MediaKind::Audio);
            Flow::Continue
        }
        // ⌥P / ^P: the palette, seeded with this document's availability.
        Outcome::Palette => {
            app.palette = Some(Palette::new(&Ctx::read(doc)));
            Flow::Continue
        }
        // ⌥H / F1: the key reference.
        Outcome::Help => {
            app.help = true;
            Flow::Continue
        }
    }
}

/// The palette's own key handling: a query line over a filtered list. Return
/// runs the highlighted command and closes; Esc closes without acting; the
/// arrows walk the list (skipping what this document can't run); everything
/// printable edits the query and re-filters.
fn handle_palette_key(doc: &mut Doc, key: KeyEvent, app: &mut App) -> Flow {
    let ctx = Ctx::read(doc);
    let palette = app.palette.as_mut().unwrap();
    match key.code {
        KeyCode::Up => palette.step(-1),
        KeyCode::Down => palette.step(1),
        KeyCode::Esc => app.palette = None,
        KeyCode::Enter => {
            // Read the choice out before dropping the palette, the same "read
            // what's needed, then clear" order the context menu uses — so the
            // command runs against a `doc` with no overlay standing over it.
            let chosen = palette.chosen();
            app.palette = None;
            if let Some(cmd) = chosen {
                let outcome = cmd.run(doc);
                return apply_outcome(doc, app, outcome);
            }
        }
        KeyCode::Backspace => {
            if let Some((i, _)) = palette.query[..palette.cursor].char_indices().next_back() {
                palette.query.drain(i..palette.cursor);
                palette.cursor = i;
                palette.refilter(&ctx);
            }
        }
        KeyCode::Left => {
            if let Some((i, _)) = palette.query[..palette.cursor].char_indices().next_back() {
                palette.cursor = i;
            }
        }
        KeyCode::Right => {
            if let Some(c) = palette.query[palette.cursor..].chars().next() {
                palette.cursor += c.len_utf8();
            }
        }
        KeyCode::Char(c) => {
            palette.query.insert(palette.cursor, c);
            palette.cursor += c.len_utf8();
            palette.refilter(&ctx);
        }
        _ => {}
    }
    Flow::Continue
}

/// A terminal paste, delivered whole rather than as the burst of key presses it
/// used to arrive as — see the `EnableBracketedPaste` note in `main`.
///
/// It routes exactly the way a key press does, by asking the same overlays in
/// the same order whether they own the keyboard. The two modal safety dialogs
/// (and the context menu, and the key reference) answer a question with a
/// keystroke and have no field for text to land in, so a paste at one of those
/// is dropped rather than allowed through to the document they exist to guard.
/// The palette and the text prompt *are* fields, so it goes into whichever is
/// open. Anything else is the document, through [`Doc::paste`] — the clipboard
/// door, not the typing one, so the whole run is one undo step and none of it
/// is read as markup somebody typed.
fn handle_paste(doc: &mut Doc, text: &str, app: &mut App) {
    if app.dirty_prompt.is_some()
        || app.conflict.is_some()
        || app.context_menu.is_some()
        || app.help
    {
        return;
    }
    if let Some(palette) = &mut app.palette {
        insert_into_field(&mut palette.query, &mut palette.cursor, text);
        palette.refilter(&Ctx::read(doc));
        return;
    }
    if let Some(prompt) = &mut app.text_prompt {
        insert_into_field(&mut prompt.value, &mut prompt.cursor, text);
        return;
    }
    doc.paste(&normalize_newlines(text));
    doc.status = Some("pasted".into());
}

/// Splice a pasted run into a single-line field at `cursor`, flattened first:
/// a prompt and the palette's query are one row tall, so a multi-line paste
/// has to become one line or the field holds characters it can never show.
fn insert_into_field(value: &mut String, cursor: &mut usize, text: &str) {
    let flat = flatten(text);
    value.insert_str(*cursor, &flat);
    *cursor += flat.len();
}

/// One line's worth of a pasted run: every line break (in any of the three
/// spellings a paste can carry) becomes a single space, and the other control
/// characters are dropped. A destination or a query with a `\t` or a `\x1b` in
/// it is a field that looks right and confirms wrong.
fn flatten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_break = false;
    for c in normalize_newlines(text).chars() {
        if c == '\n' {
            if !last_was_break {
                out.push(' ');
            }
            last_was_break = true;
        } else if !c.is_control() {
            out.push(c);
            last_was_break = false;
        }
    }
    out
}

/// A pasted run with its line endings normalized to `\n`.
///
/// Pasted text is bytes from somewhere else, and both `\r\n` (anything that
/// came through Windows) and a lone `\r` (a terminal that translates, or a
/// classic-Mac file) would otherwise be spliced into the source verbatim — a
/// carriage return the document acquires by being pasted into, which neither
/// view draws and the next save writes to disk.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn handle_mouse(doc: &mut Doc, m: MouseEvent, app: &mut App) {
    // The menu owns the mouse while it's open. Motion (with no button, or a
    // drag) hovers: the row under the pointer becomes the highlight, and moving
    // onto a submenu row opens its flyout while moving off it closes any deeper
    // one. A press runs the row's action (or opens its submenu); a press outside
    // every level dismisses the menu. Either way the event doesn't fall through
    // to the document underneath.
    if app.context_menu.is_some() {
        let ctx = Ctx::read(doc);
        let menu = app.context_menu.as_mut().unwrap();
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if let Some((lvl, idx)) = menu.hit(m.row, m.column)
                    && menu.levels[lvl].items[idx].selectable(&ctx)
                {
                    // Close any deeper flyout first, then highlight the row —
                    // and reopen its submenu if that's what it is.
                    menu.levels.truncate(lvl + 1);
                    menu.levels[lvl].selected = idx;
                    if let MenuEntry::Submenu(_, items) = menu.levels[lvl].items[idx] {
                        menu.open_submenu(lvl, items, &ctx);
                    }
                }
            }
            MouseEventKind::Down(_) => match menu.hit(m.row, m.column) {
                Some((lvl, idx)) => match menu.levels[lvl].items[idx] {
                    // A dimmed row is not a row: clicking one holds the menu
                    // open rather than closing it on an action that didn't run.
                    MenuEntry::Action(cmd) if cmd.enabled(&ctx) => {
                        app.context_menu = None;
                        let outcome = cmd.run(doc);
                        apply_outcome(doc, app, outcome);
                    }
                    MenuEntry::Submenu(_, items) if !items.is_empty() => {
                        menu.open_submenu(lvl, items, &ctx)
                    }
                    _ => {}
                },
                None => app.context_menu = None,
            },
            _ => {}
        }
        return;
    }

    // The help card and the palette own the mouse the same way the menu does.
    // Anywhere on the card dismisses the help; in the palette, a press on a
    // runnable row runs it and a press outside dismisses.
    if app.help {
        if matches!(m.kind, MouseEventKind::Down(_)) {
            app.help = false;
        }
        return;
    }
    if app.palette.is_some() {
        if matches!(m.kind, MouseEventKind::Down(_)) {
            let palette = app.palette.as_ref().unwrap();
            let chosen = palette
                .hit(m.row, m.column)
                .filter(|row| row.enabled)
                .map(|row| row.command);
            match chosen {
                Some(cmd) => {
                    app.palette = None;
                    let outcome = cmd.run(doc);
                    apply_outcome(doc, app, outcome);
                }
                // A press inside the list but on a dimmed row holds the palette
                // open; one outside it entirely dismisses.
                None if palette.covers(m.row, m.column) => {}
                None => app.palette = None,
            }
        }
        return;
    }

    // No overlay owns the mouse, so the editing surface handles it — caret
    // placement, word/block/drag selection, and scroll all happen inside the
    // widget. A right-click is the one thing it hands back: the host owns the
    // context menu it anchors.
    match leaf_ratatui::handle_mouse(doc, m, &mut app.editor) {
        MouseOutcome::Continue => {}
        MouseOutcome::ContextMenu { x, y } => {
            let ctx = Ctx::read(doc);
            app.context_menu = Some(ContextMenu::new((x, y), &ctx));
        }
    }
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

/// ⌥l: set the language of the fenced code block the caret is in, prefilled
/// with its current language — the code-block analogue of ⌥k's link prompt,
/// editing the fence's info string through a prompt rather than exposing the
/// fence markup as an editable row. A no-op (no prompt) when the caret is in no
/// fenced block, since there's nothing to label.
fn open_language_prompt(doc: &mut Doc, app: &mut App) {
    if !doc.caret_in_fenced_code() {
        return;
    }
    let initial = doc.code_language_at_caret().unwrap_or_default();
    app.text_prompt = Some(TextPrompt::new("Code language", initial, |doc, lang| {
        doc.set_code_language(lang);
    }));
}

/// ⌥e and the Insert menu's three media rows: prompt for a source, then embed
/// it. One prompt shape for all three kinds because they differ only in the tag
/// they write — and the *alt* text isn't asked for at all, because core already
/// takes it from the selection (select the caption, press ⌥e, get an image
/// captioned with it). A second field for something the gesture already has a
/// better answer to would be a field left empty every time.
///
/// The confirm callback is a plain `fn` pointer, so the kind can't be captured
/// and each gets its own — three lines that keep `TextPrompt` free of a closure
/// type it would otherwise have to be generic over.
fn open_media_prompt(app: &mut App, kind: MediaKind) {
    let (label, confirm): (&'static str, fn(&mut Doc, &str)) = match kind {
        MediaKind::Image => ("Image source", |doc, dest| doc.insert_image(dest, "")),
        MediaKind::Video => ("Video source", |doc, dest| {
            doc.insert_media(MediaKind::Video, dest, "")
        }),
        MediaKind::Audio => ("Audio source", |doc, dest| {
            doc.insert_media(MediaKind::Audio, dest, "")
        }),
    };
    app.text_prompt = Some(TextPrompt::new(label, String::new(), confirm));
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

/// Copy the current selection to the system clipboard, in both flavors.
fn clipboard_copy(doc: &mut Doc) {
    let Some(text) = doc.selected_text().map(str::to_string) else {
        doc.status = Some("nothing selected".into());
        return;
    };
    let html = doc.selection_html();
    doc.status = Some(match set_clipboard(text, html) {
        Ok(()) => "copied".into(),
        Err(_) => "clipboard unavailable".into(),
    });
}

/// Copy the current selection to the system clipboard, then delete it.
fn clipboard_cut(doc: &mut Doc) {
    let Some(text) = doc.selected_text().map(str::to_string) else {
        doc.status = Some("nothing selected".into());
        return;
    };
    let html = doc.selection_html();
    match set_clipboard(text, html) {
        Ok(()) => {
            doc.insert(""); // replaces the (still active) selection with nothing
            doc.status = Some("cut".into());
        }
        Err(_) => doc.status = Some("clipboard unavailable".into()),
    }
}

/// Insert the clipboard at the caret, preferring its rich flavor: HTML carries
/// the formatting a `text/plain` copy out of another app has already lost.
///
/// Falls through to plain on every kind of no — no HTML on the pasteboard, or
/// HTML that [`Doc::paste_html`] won't convert (see `leaf_core::html`) — because
/// the two flavors describe the same content and the plain one always exists.
fn clipboard_paste(doc: &mut Doc) {
    if let Ok(html) = get_clipboard_html()
        && doc.paste_html(&html)
    {
        doc.status = Some("pasted".into());
        return;
    }
    clipboard_paste_plain(doc);
}

/// Insert the clipboard's plain flavor, whatever else it carries (⌥V) — the
/// escape hatch for pasting the *source* of something rich.
fn clipboard_paste_plain(doc: &mut Doc) {
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
// changes. These helpers collapse arboard's error type so callers only need to
// decide between a status message and a panic (never the latter).

/// Publish both flavors. `html` is optional and `plain` is not: a selection that
/// doesn't render is still text the user asked for, and arboard writes the two
/// in one clear-and-set, so this can't leave a stale flavor behind from an
/// earlier copy for a paste to find and prefer.
fn set_clipboard(plain: String, html: Option<String>) -> Result<(), arboard::Error> {
    let mut clipboard = arboard::Clipboard::new()?;
    match html {
        Some(html) => clipboard.set().html(html, Some(plain)),
        None => clipboard.set_text(plain),
    }
}

fn get_clipboard_text() -> Result<String, arboard::Error> {
    arboard::Clipboard::new()?.get_text()
}

fn get_clipboard_html() -> Result<String, arboard::Error> {
    arboard::Clipboard::new()?.get().html()
}

#[cfg(test)]
mod tests {
    use super::*;
    use leaf_core::{InlineKind, View};
    // Modifiers/buttons the non-test code no longer references directly (the
    // editing dispatch moved into leaf-ratatui), but the test event builders do.
    use ratatui::crossterm::event::{KeyModifiers, MouseButton};

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

    fn moved(row: u16, col: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn keyp(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn clicking_a_checkbox_ticks_it_and_leaves_the_caret_alone() {
        // Body origin is (0, 1), so screen row 1 is the first document row and
        // column 0 is the box glyph itself.
        let mut doc = doc_with("task_click", "- [ ] one\n- [ ] two\n");
        let mut app = App::default();
        doc.caret = doc.source.find("two").unwrap();
        let before = doc.caret;

        handle_mouse(&mut doc, left_down(1, 0), &mut app);
        assert_eq!(doc.source, "- [x] one\n- [ ] two\n");
        assert_eq!(
            doc.caret, before,
            "ticking a box elsewhere must not move the caret"
        );

        // Clicking the item's *text* is an ordinary caret placement, not a tick.
        handle_mouse(&mut doc, left_down(1, 4), &mut app);
        assert_eq!(
            doc.source, "- [x] one\n- [ ] two\n",
            "the text is not a box"
        );
        assert_ne!(doc.caret, before, "the caret moved to the click");
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.context_menu.is_none());
        assert_eq!(doc.selection(), None);
    }

    #[test]
    fn context_menu_arrows_and_enter_run_the_highlighted_action() {
        let mut doc = doc_with("menu_nav", "one two three\n");
        // With something selected, Cut and Copy are live and the highlight opens
        // on Cut — three Downs from there lands on Select All.
        doc.anchor = Some(0);
        doc.caret = 3;
        let mut app = App::default();
        handle_mouse(&mut doc, right_down(1, 4), &mut app);
        for _ in 0..3 {
            handle_key(
                &mut doc,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &mut app,
            );
        }
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );
        assert!(app.context_menu.is_none());
        assert_eq!(doc.selected_text(), Some("one two three\n"));
    }

    #[test]
    fn menu_click_on_an_item_runs_it_and_a_click_elsewhere_just_dismisses() {
        let mut doc = doc_with("menu_click", "one two three\n");
        let mut app = App::default();
        handle_mouse(&mut doc, right_down(1, 4), &mut app);
        // The menu hasn't been drawn (no `ui::render` in this test), so no level
        // has a painted rect to hit-test; a click anywhere just dismisses it.
        assert!(app.context_menu.as_ref().unwrap().levels[0].rect.is_none());
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
        assert_eq!(
            &doc.source[..7],
            "Title\n\n",
            "first ⌥1 should strip the heading marker"
        );

        handle_key(&mut doc, alt_1, &mut app);
        assert!(
            doc.source.starts_with("# Title"),
            "second ⌥1 should re-apply H1"
        );
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut app,
        );
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &mut app,
        );
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &mut app,
        );
        assert_eq!(doc.selection(), None, "^A must not have reached select_all");
        handle_key(&mut doc, alt('b'), &mut app);
        assert_eq!(
            doc.source, "hello\n",
            "⌥b must not have reached the document"
        );
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
        assert!(
            doc.source.contains("- - b"),
            "the second item should have nested: {:?}",
            doc.source
        );
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
        let prompt = app
            .dirty_prompt
            .as_ref()
            .expect("a dirty ^Q should open the prompt");
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

    /// A path in the temp dir that nothing has written — `leaf notes.md` for a
    /// file that isn't there.
    fn missing_tui_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_tui_test_new_{name}.md"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn ctrl_s_on_a_file_that_didnt_exist_writes_it_with_no_save_as_prompt() {
        // The whole point of `open_or_create` over `blank`: the buffer already
        // knows its name, so ^S is a plain save. A Save As box here would be
        // asking the user for a path they typed on the command line.
        let p = missing_tui_path("ctrl_s");
        let mut doc = Doc::open_or_create(p.clone()).unwrap();
        doc.insert("typed into a file that didn't exist\n");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);

        assert!(app.text_prompt.is_none(), "no Save As detour");
        assert!(app.conflict.is_none(), "and nothing to conflict with");
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "typed into a file that didn't exist\n"
        );
        assert!(!doc.dirty);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn quitting_a_new_file_untouched_leaves_the_disk_alone() {
        // Nothing was typed, so there is nothing to lose and nothing to write:
        // no dirty prompt, and the file the user named still doesn't exist.
        let p = missing_tui_path("quit_untouched");
        let mut doc = Doc::open_or_create(p.clone()).unwrap();
        let mut app = App::default();
        assert!(handle_key(&mut doc, ctrl('q'), &mut app) == Flow::Quit);
        assert!(app.dirty_prompt.is_none());
        assert!(!p.exists(), "quitting must not create the file");
    }

    #[test]
    fn a_new_file_created_underneath_us_still_gets_the_overwrite_prompt() {
        // Somebody else writes the file between launch and save. That's the same
        // clobber the conflict prompt guards for an opened document, and a new
        // buffer must not be exempt from it just because it started empty.
        let p = missing_tui_path("conflict");
        let mut doc = Doc::open_or_create(p.clone()).unwrap();
        doc.insert("ours\n");
        std::fs::write(&p, "theirs\n").unwrap();

        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);
        assert!(
            app.conflict.is_some(),
            "a save about to overwrite someone's file has to ask"
        );
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "theirs\n",
            "and must not have written anything yet"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn ctrl_s_on_an_untitled_document_routes_to_save_as_instead_of_failing() {
        let mut doc = Doc::blank().unwrap();
        doc.insert("hello");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('s'), &mut app);
        let prompt = app
            .text_prompt
            .as_ref()
            .expect("^S on an untitled doc should open Save As");
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );

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
        assert!(
            app.text_prompt.is_some(),
            "Save should have detoured to Save As"
        );
        assert!(app.dirty_prompt.is_none());

        let mut p = std::env::temp_dir();
        p.push("leaf_tui_test_quit_via_saveas.md");
        let _ = std::fs::remove_file(&p);
        for c in p.to_string_lossy().chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        let flow = handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut app,
        );
        assert!(
            flow == Flow::Quit,
            "the pending quit should fire once the save-as lands"
        );
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
        assert!(
            handle_key(
                &mut doc,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                &mut app
            ) == Flow::Continue
        );
        assert!(app.text_prompt.is_none());
        assert!(
            app.dirty_prompt.is_none(),
            "canceling the destination shouldn't resurrect the quit prompt"
        );
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
        let prompt = app
            .dirty_prompt
            .as_ref()
            .expect("⌥n on a dirty doc should ask first");
        assert!(prompt.action == DirtyAction::New);
        assert_eq!(
            doc.source, "hello world\n",
            "nothing should change before the choice is made"
        );
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
        let prompt = app
            .conflict
            .as_ref()
            .expect("a changed file should stop the save");
        assert_eq!(
            prompt.selected, 2,
            "the safe default is Cancel, not Overwrite"
        );
        assert_eq!(
            std::fs::read_to_string(&doc.path).unwrap(),
            "someone else's edit\n"
        );
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(doc.source, "  line one\n");
    }

    #[test]
    fn shift_tab_outdents_one_level() {
        let mut doc = doc_with("outdent", "    line one\n");
        let mut app = App::default();
        doc.caret = 0;
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &mut app,
        );
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
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut app,
        );
        assert_eq!(
            doc.source, "| a | b |\n| - | - |\n| c | d |\n",
            "a table hop must not indent"
        );
        // A hop lands with the destination cell selected (caret at its end), so
        // typing replaces the cell — the same field-select Tab gives everywhere.
        assert_eq!(
            doc.selected_text(),
            Some("b"),
            "the hopped-to cell comes up selected"
        );
        assert_eq!(doc.caret, doc.source.find('b').unwrap() + 1);
    }

    #[test]
    fn alt_enter_in_a_markdown_table_inserts_an_in_cell_line_break() {
        // The GUI's Shift+Enter is indistinguishable from Enter in a terminal, so
        // the TUI spells the in-cell break Alt+Enter. In a Markdown cell it splices
        // the `<br>` twig reads back as a hard_break; the caret stays in the table.
        let mut doc = doc_with("table_break", "| a | b |\n| - | - |\n| c | d |\n");
        let mut app = App::default();
        doc.caret = doc.source.find('a').unwrap() + 1; // just after "a"
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &mut app,
        );
        assert_eq!(doc.source, "| a<br> | b |\n| - | - |\n| c | d |\n");
        assert!(
            doc.caret_in_table(),
            "still editing the cell, past the break"
        );
    }

    #[test]
    fn alt_enter_off_a_table_is_an_ordinary_newline() {
        let mut doc = doc_with("break_newline", "hello world\n");
        let mut app = App::default();
        doc.caret = 5; // after "hello"
        let breaks = doc.source.matches('\n').count();
        handle_key(
            &mut doc,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
            &mut app,
        );
        // Off a table `cell_line_break` declines and we fall through to the
        // ordinary newline (which opens a paragraph), so the line count grows and
        // no `<br>` is spliced.
        assert!(
            doc.source.matches('\n').count() > breaks,
            "a newline off a table"
        );
        assert!(!doc.source.contains("<br>"), "no in-cell break off a table");
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
        assert!(
            doc.active_inline_marks().contains(InlineKind::Delete),
            "status: {:?}",
            doc.status
        );
    }

    #[test]
    fn alt_u_toggles_underline_on_the_selection() {
        let mut doc = doc_with_dj("underline", "hello world\n");
        let mut app = App::default();
        doc.anchor = Some(0);
        doc.caret = 5; // "hello" selected
        handle_key(&mut doc, alt('u'), &mut app);
        assert!(
            doc.active_inline_marks().contains(InlineKind::Insert),
            "status: {:?}",
            doc.status
        );
    }

    // ── paste ────────────────────────────────────────────────────────────────

    #[test]
    fn clipboard_paste_uses_doc_paste_not_doc_insert() {
        let _clip = clipboard_lock();
        // `Doc::paste` (unlike `insert`) is always its own undo step, even for
        // one character — the observable difference is that a paste right
        // after typing does *not* coalesce into that typing run's undo.
        let mut doc = doc_with("paste_coalesce", "");
        doc.insert("a"); // a one-character typing run
        set_clipboard("b".into(), None).ok(); // best-effort: skip if no clipboard
        clipboard_paste(&mut doc);
        if doc.status.as_deref() == Some("clipboard unavailable") {
            return; // headless CI/sandbox with no system clipboard
        }
        assert_eq!(doc.source, "ab");
        doc.undo();
        assert_eq!(doc.source, "a", "undo should peel off only the pasted 'b'");
    }

    /// Put both flavors on the pasteboard, or `false` when there isn't one to
    /// put them on — the clipboard tests run wherever the suite does, including
    /// a headless box with no pasteboard at all, and a skip beats a flake.
    fn seed_clipboard(plain: &str, html: &str) -> bool {
        set_clipboard(plain.into(), Some(html.into())).is_ok()
    }

    /// The system pasteboard is one object shared by the whole machine, and the
    /// test runner is threaded: two tests in it at once is not a flake but a
    /// SIGSEGV out of AppKit, and even without the crash they would read each
    /// other's clipboard and pass for the wrong reason. Every test that touches
    /// the real pasteboard takes this first.
    ///
    /// The app itself needs no such lock — a frontend does clipboard work on the
    /// one thread its event loop runs on.
    static CLIPBOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clipboard_lock() -> std::sync::MutexGuard<'static, ()> {
        // A test that panics mid-clipboard poisons this; the data is `()`, so
        // there is no invariant left broken for the next test to trip over.
        CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn paste_prefers_the_html_flavor_and_converts_it() {
        let _clip = clipboard_lock();
        let mut doc = doc_with("paste_rich", "");
        if !seed_clipboard("bold", "<p>a <strong>b</strong> c</p>") {
            return; // no pasteboard here
        }
        clipboard_paste(&mut doc);
        if doc.status.as_deref() == Some("clipboard unavailable") {
            return;
        }
        assert_eq!(doc.source, "a **b** c", "the rich flavor, as Markdown");
    }

    #[test]
    fn paste_falls_back_to_plain_when_the_html_will_not_convert() {
        let _clip = clipboard_lock();
        let mut doc = doc_with("paste_fallback", "");
        // twig builds no table from HTML: the plain flavor is the better answer.
        if !seed_clipboard("a\tb", "<table><tr><td>a</td><td>b</td></tr></table>") {
            return;
        }
        clipboard_paste(&mut doc);
        if doc.status.as_deref() == Some("clipboard unavailable") {
            return;
        }
        assert_eq!(doc.source, "a\tb");
    }

    #[test]
    fn alt_v_pastes_the_plain_flavor_even_when_html_is_there() {
        let _clip = clipboard_lock();
        let mut doc = doc_with("paste_plain", "");
        let mut app = App::default();
        if !seed_clipboard("a **b** c", "<p>a <strong>b</strong> c</p>") {
            return;
        }
        handle_key(&mut doc, alt('v'), &mut app);
        if doc.status.as_deref() == Some("clipboard unavailable") {
            return;
        }
        assert_eq!(doc.source, "a **b** c", "the source, not the rich flavor");
    }

    #[test]
    fn copy_publishes_both_flavors() {
        let _clip = clipboard_lock();
        let mut doc = doc_with("copy_both", "a **bold** c\n");
        doc.anchor = Some(2);
        doc.caret = 10; // `**bold**`
        clipboard_copy(&mut doc);
        if doc.status.as_deref() == Some("clipboard unavailable") {
            return;
        }
        assert_eq!(
            get_clipboard_text().ok().as_deref(),
            Some("**bold**"),
            "the source"
        );
        let html = get_clipboard_html().expect("html flavor");
        assert!(html.contains("<strong>bold</strong>"), "{html:?}");
    }

    // ── bracketed paste ──────────────────────────────────────────────────────

    #[test]
    fn a_bracketed_paste_lands_as_one_undo_step_not_a_run_of_keystrokes() {
        // The whole point of routing `Event::Paste` to `Doc::paste`: before it,
        // a paste was a burst of `KeyEvent`s and so a burst of `Doc::insert`s —
        // one undo step per character, folded into whatever typing run preceded
        // it. Now the run peels off in one.
        let mut doc = doc_with("bracketed_undo", "");
        let mut app = App::default();
        doc.insert("a"); // a one-character typing run to fold into
        handle_paste(&mut doc, "one\ntwo", &mut app);
        assert_eq!(doc.source, "aone\ntwo");
        doc.undo();
        assert_eq!(doc.source, "a", "the whole paste comes off in one step");
    }

    #[test]
    fn a_pasted_list_marker_is_not_read_as_typing_a_list() {
        // `- ` typed at the start of a line is an autoformat gesture; the same
        // two characters arriving inside a paste are text. `Doc::paste` splices
        // rather than interpreting, which is exactly the difference.
        let mut doc = doc_with("bracketed_literal", "");
        let mut app = App::default();
        handle_paste(&mut doc, "- one\n- two\n", &mut app);
        assert_eq!(doc.source, "- one\n- two\n");
    }

    #[test]
    fn a_pasted_crlf_run_lands_as_plain_newlines() {
        let mut doc = doc_with("bracketed_crlf", "");
        let mut app = App::default();
        handle_paste(&mut doc, "one\r\ntwo\rthree", &mut app);
        assert_eq!(
            doc.source, "one\ntwo\nthree",
            "no carriage return should reach the source"
        );
    }

    #[test]
    fn a_paste_goes_into_the_text_prompt_rather_than_the_document() {
        let mut doc = doc_with("bracketed_prompt", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        handle_paste(&mut doc, "https://example.com", &mut app);
        assert_eq!(
            app.text_prompt.as_ref().unwrap().value,
            "https://example.com"
        );
        assert_eq!(doc.source, "hello\n", "and not into the document");
    }

    #[test]
    fn a_multi_line_paste_into_a_one_row_field_flattens_to_spaces() {
        let mut doc = doc_with("bracketed_flatten", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('k'), &mut app);
        handle_paste(&mut doc, "one\r\n\ttwo\nthree", &mut app);
        assert_eq!(app.text_prompt.as_ref().unwrap().value, "one two three");
    }

    #[test]
    fn a_paste_into_the_palette_narrows_the_query() {
        let mut doc = doc_with("bracketed_palette", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('p'), &mut app);
        handle_paste(&mut doc, "bold", &mut app);
        let palette = app
            .palette
            .as_ref()
            .expect("the palette should still be up");
        assert_eq!(palette.query, "bold");
        assert_eq!(palette.rows[palette.selected].command.label(), "Bold");
    }

    #[test]
    fn a_paste_while_a_modal_prompt_is_up_is_ignored() {
        // The dirty prompt is guarding the document; a paste that leaked past it
        // would edit exactly the text the dialog is asking about.
        let mut doc = doc_with("bracketed_modal", "hello\n");
        doc.caret = 5;
        doc.insert(" world");
        let mut app = App::default();
        handle_key(&mut doc, ctrl('q'), &mut app);
        handle_paste(&mut doc, "nope", &mut app);
        assert_eq!(doc.source, "hello world\n");
        assert!(app.dirty_prompt.is_some(), "and the prompt stays up");
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
        assert!(
            doc.scroll > before,
            "dragging past the bottom edge should scroll down"
        );
        assert!(
            doc.selection().is_some(),
            "the drag should still be extending a selection"
        );
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

    // ── chrome-less rendering ────────────────────────────────────────────────

    use ratatui::{Terminal, backend::TestBackend};

    /// Draw one frame of the whole UI at `w`×`h` and read the screen back as
    /// rows of text — the host chrome's own render path, exercised end to end.
    fn frame(doc: &mut Doc, app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::render(f, doc, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_body_starts_on_the_top_row_now_that_there_is_no_header() {
        // The document used to open under a one-row header; with the chrome gone
        // its first line is the terminal's first row.
        let mut doc = doc_with("no_header", "hello world\n");
        let mut app = App::default();
        let lines = frame(&mut doc, &mut app, 40, 6);
        assert!(
            lines[0].starts_with("hello world"),
            "body not on row 0:\n{}",
            lines.join("\n")
        );
    }

    #[test]
    fn a_dirty_prompt_floats_as_a_centered_overlay() {
        let mut doc = doc_with("prompt_overlay", "hello\n");
        let mut app = App {
            dirty_prompt: Some(DirtyPrompt {
                action: DirtyAction::Quit,
                selected: 0,
            }),
            ..Default::default()
        };
        let lines = frame(&mut doc, &mut app, 50, 10);
        let joined = lines.join("\n");
        assert!(joined.contains("Unsaved changes"), "no dialog:\n{joined}");
        assert!(
            joined.contains("Save") && joined.contains("Discard"),
            "no choices:\n{joined}"
        );
        // Centered, not pinned to the bottom rows the old footer used.
        let row = lines
            .iter()
            .position(|l| l.contains("Unsaved changes"))
            .unwrap();
        assert!(
            row > 0 && row < 9,
            "dialog should be centered, got row {row}"
        );
    }

    #[test]
    fn a_status_message_shows_as_a_bottom_right_toast() {
        let mut doc = doc_with("toast", "hello\n");
        doc.status = Some("copied".into());
        let mut app = App::default();
        let lines = frame(&mut doc, &mut app, 40, 6);
        let bottom = lines.last().unwrap();
        assert!(
            bottom.contains("copied"),
            "toast missing from bottom row:\n{}",
            lines.join("\n")
        );
        // The toast is drawn flush against the right edge (its text is padded
        // with a single trailing space), and the space to its left is empty body.
        assert!(
            bottom.ends_with("copied "),
            "toast should hug the right edge: {bottom:?}"
        );
        assert!(
            bottom.starts_with("     "),
            "toast should not stretch across the row: {bottom:?}"
        );
    }

    #[test]
    fn a_dirty_prompt_suppresses_the_status_toast() {
        // A dialog and a toast shouldn't fight for the same glance: while the
        // dialog is up, the toast stays hidden.
        let mut doc = doc_with("no_toast_with_prompt", "hello\n");
        doc.status = Some("copied".into());
        let mut app = App {
            dirty_prompt: Some(DirtyPrompt {
                action: DirtyAction::Quit,
                selected: 0,
            }),
            ..Default::default()
        };
        let lines = frame(&mut doc, &mut app, 40, 6);
        assert!(
            !lines.join("\n").contains("copied"),
            "toast should be suppressed:\n{}",
            lines.join("\n")
        );
    }

    // ── context menu: Format submenu, hover, active state ────────────────────

    /// Walk the frontmost menu level's highlight onto the row called `label`,
    /// pressing Down until it lands there. By name rather than by a count of
    /// keystrokes: rows are now dimmed by the document's format and the caret's
    /// surroundings, so which index the *n*th Down reaches is a property of the
    /// document, and a test that hard-codes it is testing the fixture.
    fn step_to(doc: &mut Doc, app: &mut App, label: &str) {
        let level = |app: &App| {
            let menu = app.context_menu.as_ref().expect("a menu should be open");
            let lvl = menu.active_level();
            (menu.levels[lvl].items, menu.levels[lvl].selected)
        };
        let (items, _) = level(app);
        for _ in 0..items.len() {
            let (items, selected) = level(app);
            if items[selected].label() == label {
                return;
            }
            handle_key(doc, keyp(KeyCode::Down), app);
        }
        let (items, selected) = level(app);
        panic!(
            "no selectable row called {label:?}; stopped on {:?}",
            items[selected].label()
        );
    }

    /// Right-click, then open the `Format` flyout.
    fn open_format(doc: &mut Doc, app: &mut App) {
        handle_mouse(doc, right_down(1, 2), app);
        step_to(doc, app, "Format");
        handle_key(doc, keyp(KeyCode::Right), app); // open the submenu
    }

    #[test]
    fn right_arrow_on_format_opens_the_styling_submenu() {
        let mut doc = doc_with("submenu_open", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        let menu = app.context_menu.as_ref().unwrap();
        assert_eq!(menu.levels.len(), 2, "Format should push a second level");
        // Its highlight starts on the first *selectable* row — past the "Block"
        // header at index 0, on Paragraph at index 1.
        assert_eq!(
            menu.levels[1].items[menu.levels[1].selected].label(),
            "Paragraph"
        );
    }

    #[test]
    fn submenu_arrows_skip_section_headers() {
        let mut doc = doc_with("submenu_headers", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        let row = |app: &App| {
            let lvl = &app.context_menu.as_ref().unwrap().levels[1];
            lvl.items[lvl.selected].label()
        };
        // Up from Paragraph wraps to the last row this *Markdown* document can
        // actually run — Code, because Markdown spells three of the eight inline
        // marks and highlight, strikethrough and underline are not among them —
        // never landing on the "Inline" header on the way.
        handle_key(&mut doc, keyp(KeyCode::Up), &mut app);
        assert_eq!(row(&app), "Code");
        // Down from there wraps past the "Block" header back to Paragraph.
        handle_key(&mut doc, keyp(KeyCode::Down), &mut app);
        assert_eq!(row(&app), "Paragraph");
    }

    #[test]
    fn choosing_bold_from_the_submenu_toggles_the_selection() {
        let mut doc = doc_with("submenu_bold", "hello world\n");
        doc.anchor = Some(0);
        doc.caret = 5; // "hello" selected
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        step_to(&mut doc, &mut app, "Bold");
        handle_key(&mut doc, keyp(KeyCode::Enter), &mut app);
        assert!(
            app.context_menu.is_none(),
            "running an action closes the menu"
        );
        assert_eq!(doc.source, "**hello** world\n");
    }

    #[test]
    fn choosing_heading_from_the_submenu_sets_the_block() {
        let mut doc = doc_with("submenu_heading", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        step_to(&mut doc, &mut app, "Heading 1");
        handle_key(&mut doc, keyp(KeyCode::Enter), &mut app);
        assert_eq!(doc.source, "# hello\n");
    }

    #[test]
    fn left_backs_out_of_the_submenu_without_closing_the_menu() {
        let mut doc = doc_with("submenu_back", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        assert_eq!(app.context_menu.as_ref().unwrap().levels.len(), 2);
        handle_key(&mut doc, keyp(KeyCode::Left), &mut app);
        let menu = app
            .context_menu
            .as_ref()
            .expect("Left in a submenu backs out, not closes");
        assert_eq!(menu.levels.len(), 1);
        // A second Left, now at the root, closes it.
        handle_key(&mut doc, keyp(KeyCode::Left), &mut app);
        assert!(app.context_menu.is_none());
    }

    #[test]
    fn hovering_a_row_highlights_it_and_hovering_format_opens_the_submenu() {
        let mut doc = doc_with("hover", "hello\n");
        // Selected, so the two clipboard rows this test hovers are live.
        doc.anchor = Some(0);
        doc.caret = 5;
        let mut app = App::default();
        handle_mouse(&mut doc, right_down(1, 2), &mut app);
        // Paint once so each level gets a rect to hit-test against.
        let _ = frame(&mut doc, &mut app, 40, 20);
        let root = app.context_menu.as_ref().unwrap().levels[0].rect.unwrap();

        // Hover Copy (root row 1): it becomes the highlight without any click.
        handle_mouse(&mut doc, moved(root.y + 1, root.x + 1), &mut app);
        assert_eq!(app.context_menu.as_ref().unwrap().levels[0].selected, 1);

        // Hover Format (root row 4): its submenu flies out on hover alone.
        handle_mouse(&mut doc, moved(root.y + 4, root.x + 1), &mut app);
        assert_eq!(
            app.context_menu.as_ref().unwrap().levels.len(),
            2,
            "hover opens the submenu"
        );

        // Hover back onto Cut (root row 0): the submenu closes again.
        handle_mouse(&mut doc, moved(root.y, root.x + 1), &mut app);
        assert_eq!(
            app.context_menu.as_ref().unwrap().levels.len(),
            1,
            "hovering off Format closes it"
        );
        assert_eq!(app.context_menu.as_ref().unwrap().levels[0].selected, 0);
    }

    #[test]
    fn the_format_submenu_renders_its_sections_and_flies_out() {
        let mut doc = doc_with("submenu_render", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        let lines = frame(&mut doc, &mut app, 60, 20);
        let joined = lines.join("\n");
        assert!(
            joined.contains("Format"),
            "the root stays visible beside the flyout:\n{joined}"
        );
        assert!(
            joined.contains('▸'),
            "the submenu arrow is drawn:\n{joined}"
        );
        assert!(
            joined.contains("Block") && joined.contains("Inline"),
            "section headers:\n{joined}"
        );
        assert!(
            joined.contains("Bold") && joined.contains("Strikethrough"),
            "inline options listed:\n{joined}"
        );
    }

    #[test]
    fn an_active_inline_style_shows_a_check_in_the_submenu() {
        // Caret inside bold text: the Bold row should carry its ✓.
        let mut doc = doc_with("submenu_active", "**hello** world\n");
        doc.anchor = Some(2);
        doc.caret = 7; // inside the bold "hello"
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        let joined = frame(&mut doc, &mut app, 60, 20).join("\n");
        assert!(
            joined.contains("✓ Bold"),
            "active Bold should be checked:\n{joined}"
        );
        assert!(
            joined.contains("  Italic"),
            "inactive Italic should not:\n{joined}"
        );
    }

    // ── the commands that had no surface before ──────────────────────────────

    /// Alt with Shift: crossterm reports the shifted letter *and* the modifier,
    /// which is how ⌥⇧W stays distinguishable from ⌥w in a terminal.
    fn alt_shift(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT | KeyModifiers::SHIFT)
    }

    #[test]
    fn alt_f_inserts_a_footnote_pair() {
        let mut doc = doc_with("footnote_insert", "a claim\n");
        let mut app = App::default();
        doc.caret = 7; // after "claim"
        handle_key(&mut doc, alt('f'), &mut app);
        assert!(
            doc.source.contains("[^1]") && doc.source.contains("[^1]:"),
            "⌥f should write both the reference and its definition:\n{}",
            doc.source
        );
    }

    #[test]
    fn alt_g_walks_a_footnote_round_trip() {
        let mut doc = doc_with("footnote_follow", "a claim[^1]\n\n[^1]: the note\n");
        let mut app = App::default();
        doc.caret = doc.source.find("[^1]").unwrap() + 2; // on the reference's label

        handle_key(&mut doc, alt('g'), &mut app);
        let note_body = doc.source.find("the note").unwrap();
        assert_eq!(doc.caret, note_body, "⌥g should land in the note's body");

        // …and again, from the note back to the reference that cites it.
        handle_key(&mut doc, alt('g'), &mut app);
        assert!(
            doc.caret < note_body,
            "⌥g in the note should go back to the reference, not deeper"
        );
    }

    #[test]
    fn alt_g_on_a_fragment_link_lands_on_the_heading_it_names() {
        let mut doc = doc_with(
            "fragment_follow",
            "see [below](#the-target)\n\n## The Target\n\nbody\n",
        );
        let mut app = App::default();
        doc.caret = doc.source.find("below").unwrap();
        handle_key(&mut doc, alt('g'), &mut app);
        assert!(
            doc.caret >= doc.source.find("## The Target").unwrap(),
            "⌥g should jump forward to the heading the fragment slugs to, not sit still"
        );
    }

    #[test]
    fn alt_g_reports_an_external_destination_rather_than_opening_it() {
        let mut doc = doc_with("external_follow", "see [docs](https://example.com)\n");
        let mut app = App::default();
        let before = doc.caret;
        doc.caret = doc.source.find("docs").unwrap();
        handle_key(&mut doc, alt('g'), &mut app);
        assert_eq!(
            doc.status.as_deref(),
            Some("→ https://example.com"),
            "an external link is named, not followed"
        );
        assert_ne!(doc.caret, before + 1000); // the caret stays in the document
    }

    #[test]
    fn alt_r_inserts_a_horizontal_rule() {
        let mut doc = doc_with("rule", "before\n");
        let mut app = App::default();
        doc.caret = 6;
        handle_key(&mut doc, alt('r'), &mut app);
        assert!(
            doc.source.contains("---") || doc.source.contains("***"),
            "⌥r should write a thematic break:\n{}",
            doc.source
        );
    }

    #[test]
    fn alt_e_opens_the_image_prompt_and_confirming_embeds_the_picture() {
        let mut doc = doc_with("image_prompt", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('e'), &mut app);
        assert_eq!(
            app.text_prompt.as_ref().map(|p| p.label),
            Some("Image source")
        );
        for c in "pic.png".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        handle_key(&mut doc, keyp(KeyCode::Enter), &mut app);
        assert!(
            doc.source.contains("pic.png"),
            "confirming should embed the image:\n{}",
            doc.source
        );
    }

    #[test]
    fn alt_shift_w_cycles_the_markup_mode_and_alt_shift_f_flips_line_flow() {
        let mut doc = doc_with("modes", "hello\n");
        let mut app = App::default();
        assert_eq!(doc.markup_mode(), MarkupMode::None);
        handle_key(&mut doc, alt_shift('W'), &mut app);
        assert_eq!(doc.markup_mode(), MarkupMode::Shortcuts);
        handle_key(&mut doc, alt_shift('W'), &mut app);
        assert_eq!(doc.markup_mode(), MarkupMode::Full);
        handle_key(&mut doc, alt_shift('W'), &mut app);
        assert_eq!(
            doc.markup_mode(),
            MarkupMode::None,
            "three notches, then round"
        );

        assert_eq!(doc.line_flow(), LineFlow::Fold);
        handle_key(&mut doc, alt_shift('F'), &mut app);
        assert_eq!(doc.line_flow(), LineFlow::Preserve);
    }

    // ── tables: the key policy, and the structural commands ──────────────────

    #[test]
    fn tab_off_the_last_cell_grows_the_table_by_a_row() {
        let mut doc = doc_with("table_grow", "| a | b |\n| - | - |\n| c | d |\n");
        let mut app = App::default();
        doc.caret = doc.source.rfind('d').unwrap(); // the last cell
        let rows_before = doc.source.lines().count();
        handle_key(&mut doc, keyp(KeyCode::Tab), &mut app);
        assert!(
            doc.source.lines().count() > rows_before,
            "Tab off the last cell should append a row:\n{}",
            doc.source
        );
    }

    #[test]
    fn return_in_a_table_drops_a_row_instead_of_splitting_the_cell() {
        let mut doc = doc_with("table_return", "| a | b |\n| - | - |\n| c | d |\n");
        let mut app = App::default();
        doc.caret = doc.source.find('c').unwrap();
        handle_key(&mut doc, keyp(KeyCode::Enter), &mut app);
        assert!(
            doc.source.starts_with("| a | b |"),
            "Return in a cell must not break the table apart:\n{}",
            doc.source
        );
    }

    #[test]
    fn the_table_commands_edit_the_grid() {
        let mut doc = doc_with("table_ops", "| a | b |\n| - | - |\n| c | d |\n");
        doc.caret = doc.source.find('c').unwrap();

        Command::RowBelow.run(&mut doc);
        assert_eq!(
            doc.source.lines().count(),
            4,
            "a row was inserted:\n{}",
            doc.source
        );

        Command::ColumnRight.run(&mut doc);
        assert_eq!(
            doc.source.lines().next().unwrap().matches('|').count(),
            4,
            "a column was inserted:\n{}",
            doc.source
        );

        Command::DeleteColumn.run(&mut doc);
        assert_eq!(
            doc.source.lines().next().unwrap().matches('|').count(),
            3,
            "and removed again:\n{}",
            doc.source
        );
    }

    // ── the palette ──────────────────────────────────────────────────────────

    #[test]
    fn alt_p_opens_the_palette_and_typing_narrows_it() {
        let mut doc = doc_with("palette_open", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('p'), &mut app);
        let all = app.palette.as_ref().unwrap().rows.len();

        for c in "rule".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        let narrowed = app.palette.as_ref().unwrap();
        assert!(narrowed.rows.len() < all, "typing should filter");
        assert_eq!(narrowed.chosen(), Some(Command::ThematicBreak));
    }

    #[test]
    fn the_palette_runs_the_chosen_command_and_closes() {
        let mut doc = doc_with("palette_run", "| a | b |\n| - | - |\n| c | d |\n");
        let mut app = App::default();
        doc.caret = doc.source.find('c').unwrap();
        handle_key(&mut doc, ctrl('p'), &mut app); // ^p opens it too
        for c in "insert row below".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        handle_key(&mut doc, keyp(KeyCode::Enter), &mut app);
        assert!(
            app.palette.is_none(),
            "running a command closes the palette"
        );
        assert_eq!(
            doc.source.lines().count(),
            4,
            "the row landed:\n{}",
            doc.source
        );
    }

    /// The palette owns the keyboard completely: a letter typed into its query
    /// must not also reach the document underneath.
    #[test]
    fn the_palette_swallows_document_keys_while_it_is_open() {
        let mut doc = doc_with("palette_capture", "hello\n");
        let before = doc.source.clone();
        let mut app = App::default();
        handle_key(&mut doc, alt('p'), &mut app);
        for c in "bold".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        handle_key(&mut doc, ctrl('s'), &mut app);
        assert_eq!(
            doc.source, before,
            "nothing should have reached the document"
        );
        handle_key(&mut doc, keyp(KeyCode::Esc), &mut app);
        assert!(app.palette.is_none());
    }

    /// A command the format cannot spell is listed and dimmed, and Return on a
    /// query that matches only such commands does nothing rather than something
    /// surprising.
    #[test]
    fn the_palette_will_not_run_a_command_this_format_cannot_spell() {
        let mut doc = doc_with("palette_gated", "hello\n"); // Markdown
        let mut app = App::default();
        let before = doc.source.clone();
        handle_key(&mut doc, alt('p'), &mut app);
        for c in "highlight".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        let palette = app.palette.as_ref().unwrap();
        assert!(
            palette
                .rows
                .iter()
                .any(|r| r.command == Command::Inline(InlineKind::Mark) && !r.enabled),
            "the djot-only highlight should be listed and dimmed in Markdown"
        );
        handle_key(&mut doc, keyp(KeyCode::Enter), &mut app);
        assert_eq!(doc.source, before);
    }

    #[test]
    fn the_palette_renders_its_query_and_rows() {
        let mut doc = doc_with("palette_render", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('p'), &mut app);
        for c in "foot".chars() {
            handle_key(&mut doc, plain(c), &mut app);
        }
        let joined = frame(&mut doc, &mut app, 70, 20).join("\n");
        assert!(joined.contains("foot"), "the query line:\n{joined}");
        assert!(joined.contains("Footnote"), "the matching row:\n{joined}");
        assert!(joined.contains("⌥f"), "its key:\n{joined}");
    }

    // ── the key reference ────────────────────────────────────────────────────

    #[test]
    fn alt_h_opens_the_key_reference_and_any_key_closes_it() {
        let mut doc = doc_with("help", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, alt('h'), &mut app);
        assert!(app.help);
        // A realistic terminal: the card lays itself out in as many columns as
        // it needs to fit this one.
        let joined = frame(&mut doc, &mut app, 100, 30).join("\n");
        assert!(joined.contains("⌥b"), "it lists keys:\n{joined}");
        assert!(
            joined.contains("palette"),
            "and names the palette:\n{joined}"
        );
        assert!(
            joined.contains("⌥⇧w"),
            "including the shifted ones:\n{joined}"
        );

        handle_key(&mut doc, keyp(KeyCode::Esc), &mut app);
        assert!(!app.help);
    }

    #[test]
    fn f1_opens_the_key_reference_too() {
        let mut doc = doc_with("help_f1", "hello\n");
        let mut app = App::default();
        handle_key(&mut doc, keyp(KeyCode::F(1)), &mut app);
        assert!(app.help);
    }

    // ── menu gating ──────────────────────────────────────────────────────────

    #[test]
    fn the_table_flyout_is_dimmed_off_a_table_and_live_inside_one() {
        let mut doc = doc_with(
            "menu_table_gate",
            "para\n\n| a | b |\n| - | - |\n| c | d |\n",
        );
        let mut app = App::default();
        // The menu is opened directly rather than by a right-click: a click also
        // *places* the caret, and where the caret is standing is the whole point
        // of this test.
        let open_at_caret = |doc: &mut Doc, app: &mut App| {
            let ctx = Ctx::read(doc);
            app.context_menu = Some(ContextMenu::new((1, 2), &ctx));
        };

        doc.caret = 1; // in the paragraph
        open_at_caret(&mut doc, &mut app);
        // Walking the whole root never lands on Table: the flyout has nothing to
        // offer off a table, so it dims and the highlight steps over it.
        for _ in 0..ROOT_MENU.len() {
            let menu = app.context_menu.as_ref().unwrap();
            assert_ne!(
                menu.levels[0].items[menu.levels[0].selected].label(),
                "Table",
                "the Table flyout should be unreachable off a table"
            );
            handle_key(&mut doc, keyp(KeyCode::Down), &mut app);
        }

        // With the caret in the table, it becomes reachable — and opens.
        doc.caret = doc.source.find('c').unwrap();
        open_at_caret(&mut doc, &mut app);
        step_to(&mut doc, &mut app, "Table");
        handle_key(&mut doc, keyp(KeyCode::Right), &mut app);
        let menu = app.context_menu.as_ref().unwrap();
        assert_eq!(menu.levels.len(), 2, "Table should push a second level");
        assert_eq!(
            menu.levels[1].items[menu.levels[1].selected].label(),
            "Insert Row Above",
            "past the \"Rows\" header, onto the first real row"
        );
    }

    #[test]
    fn a_dimmed_row_is_drawn_but_not_run_by_a_click() {
        // Markdown spells no highlight, so the Format flyout's Highlight row is
        // dimmed — and clicking it must leave both the menu and the document as
        // they were.
        let mut doc = doc_with("menu_dimmed_click", "hello\n");
        doc.anchor = Some(0);
        doc.caret = 5;
        let mut app = App::default();
        let before = doc.source.clone();
        open_format(&mut doc, &mut app);
        let _ = frame(&mut doc, &mut app, 70, 30);

        let rect = app.context_menu.as_ref().unwrap().levels[1].rect.unwrap();
        let row = FORMAT_MENU
            .iter()
            .position(|e| e.label() == "Highlight")
            .unwrap() as u16;
        handle_mouse(&mut doc, left_down(rect.y + row, rect.x + 1), &mut app);
        assert!(
            app.context_menu.is_some(),
            "a click on a dimmed row holds the menu open"
        );
        assert_eq!(doc.source, before);
    }

    // ── the hover peek ───────────────────────────────────────────────────────

    #[test]
    fn hovering_a_footnote_reference_shows_the_note_without_moving_the_caret() {
        let mut doc = doc_with("peek_footnote", "a claim[^1]\n\n[^1]: the note itself\n");
        doc.view = leaf_core::View::Wysiwyg;
        doc.build_visual(80);
        let mut app = App::default();
        doc.caret = 0;

        // The rendered row/col of the reference, found through the same map the
        // pointer hit-test uses.
        let off = doc.source.find("[^1]").unwrap() + 2;
        let (row, col) = doc.vmap.pos_of_offset(off);
        handle_mouse(&mut doc, moved(row as u16 + 1, col as u16), &mut app);

        assert_eq!(doc.caret, 0, "a peek must not move the caret");
        let status = doc.status.clone().unwrap_or_default();
        assert!(
            status.contains("the note itself"),
            "hovering should preview the note, got {status:?}"
        );
    }

    /// Moving off the reference takes the peek back down — but a status this
    /// module didn't put up survives the pointer wandering over it.
    #[test]
    fn a_peek_clears_itself_and_leaves_other_statuses_alone() {
        let mut doc = doc_with(
            "peek_clear",
            "a claim[^1] and more text here\n\n[^1]: note\n",
        );
        doc.view = leaf_core::View::Wysiwyg;
        doc.build_visual(80);
        let mut app = App::default();

        let off = doc.source.find("[^1]").unwrap() + 2;
        let (row, col) = doc.vmap.pos_of_offset(off);
        handle_mouse(&mut doc, moved(row as u16 + 1, col as u16), &mut app);
        assert!(doc.status.is_some());

        // Off the reference, onto ordinary text.
        let plain_off = doc.source.find("more").unwrap();
        let (prow, pcol) = doc.vmap.pos_of_offset(plain_off);
        handle_mouse(&mut doc, moved(prow as u16 + 1, pcol as u16), &mut app);
        assert_eq!(doc.status, None, "the peek should come back down");

        // A status from elsewhere is not the peek's to clear.
        doc.status = Some("saved".into());
        handle_mouse(&mut doc, moved(prow as u16 + 1, pcol as u16 + 1), &mut app);
        assert_eq!(doc.status.as_deref(), Some("saved"));
    }

    #[test]
    fn hovering_a_link_shows_where_it_points() {
        let mut doc = doc_with("peek_link", "see [docs](https://example.com) here\n");
        doc.view = leaf_core::View::Wysiwyg;
        doc.build_visual(80);
        let mut app = App::default();

        let off = doc.source.find("docs").unwrap();
        let (row, col) = doc.vmap.pos_of_offset(off);
        handle_mouse(&mut doc, moved(row as u16 + 1, col as u16), &mut app);
        assert_eq!(doc.status.as_deref(), Some("→ https://example.com"));
    }

    // ── chrome colors ────────────────────────────────────────────────────────

    /// Paint `overlay` in `scheme` and hand back every cell of the frame.
    use ratatui::style::Color;

    fn chrome_cells(
        scheme: leaf_core::ColorScheme,
        overlay: impl Fn(&mut Doc, &mut App),
    ) -> ratatui::buffer::Buffer {
        let mut doc = doc_with("chrome", "hello\n");
        let mut app = App::default();
        app.editor.set_color_scheme(scheme);
        overlay(&mut doc, &mut app);
        let mut term = Terminal::new(TestBackend::new(70, 24)).unwrap();
        term.draw(|f| ui::render(f, &mut doc, &mut app)).unwrap();
        term.backend().buffer().clone()
    }

    /// The bug this guards: the host chrome used to spell its colors as ANSI
    /// constants — a dark-grey panel with white text — while the editing surface
    /// underneath read a light-or-dark palette from the terminal. On a light
    /// terminal that painted the menu and the palette as a near-black slab, and
    /// the grey and yellow on top of it were the two least readable colors
    /// available. Chrome that doesn't move when the scheme does is the failure.
    #[test]
    fn the_chrome_follows_the_terminal_s_light_or_dark_scheme() {
        let open_palette = |doc: &mut Doc, app: &mut App| {
            handle_key(doc, alt('p'), app);
        };
        let light = chrome_cells(leaf_core::ColorScheme::Light, open_palette);
        let dark = chrome_cells(leaf_core::ColorScheme::Dark, open_palette);

        // The row under the palette's highlight, in each scheme.
        let panel = |buf: &ratatui::buffer::Buffer| {
            (0..buf.area.height)
                .map(|y| buf[(12, y)].clone())
                .find(|c| c.bg != ratatui::style::Color::Reset)
                .expect("the palette should paint a panel")
        };
        let (l, d) = (panel(&light), panel(&dark));
        assert_ne!(l.bg, d.bg, "the panel fill must differ between schemes");
        assert_ne!(l.fg, d.fg, "and so must the text on it");
    }

    /// No overlay may paint a cell whose text is the same color as the ground
    /// under it. Cheap, and it catches the whole family of "this row turned out
    /// invisible in one of the two schemes" — which is how the chrome broke in
    /// the first place.
    #[test]
    fn no_overlay_paints_invisible_text() {
        /// One overlay to paint, and the name to blame if it paints badly.
        type Overlay = (&'static str, fn(&mut Doc, &mut App));

        let overlays: [Overlay; 4] = [
            ("palette", |doc, app| {
                handle_key(doc, alt('p'), app);
            }),
            ("key reference", |doc, app| {
                handle_key(doc, alt('h'), app);
            }),
            ("context menu", |doc, app| {
                handle_mouse(doc, right_down(1, 2), app);
            }),
            ("unsaved-changes dialog", |doc, app| {
                doc.insert("x"); // make it dirty, so ^q asks rather than quitting
                handle_key(doc, ctrl('q'), app);
            }),
        ];
        for scheme in [leaf_core::ColorScheme::Light, leaf_core::ColorScheme::Dark] {
            for (name, overlay) in overlays {
                let buf = chrome_cells(scheme, overlay);
                for y in 0..buf.area.height {
                    for x in 0..buf.area.width {
                        let cell = &buf[(x, y)];
                        // Only the cells the chrome actually painted. A cell
                        // left on the terminal's own defaults is the document
                        // showing through, and those two contrast by definition
                        // — it's the user's own theme.
                        if cell.bg == Color::Reset || cell.symbol().trim().is_empty() {
                            continue;
                        }
                        assert_ne!(
                            cell.fg,
                            cell.bg,
                            "{name} in {scheme:?} paints {:?} invisibly at ({x}, {y})",
                            cell.symbol()
                        );
                        // An explicit fill with an inherited foreground is the
                        // same bug one step less obvious: the terminal's own
                        // text color is dark on a light terminal and light on a
                        // dark one, and our panel is neither.
                        assert_ne!(
                            cell.fg,
                            Color::Reset,
                            "{name} in {scheme:?} leaves {:?} at ({x}, {y}) on the \
                             terminal's foreground over a panel fill",
                            cell.symbol()
                        );
                    }
                }
            }
        }
    }
}
