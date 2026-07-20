//! leaf — a caret-based rich-text TUI editor for documents, built on twig.
//!
//! Sibling to bough: same twig backend, opposite interaction model. bough moves
//! a selection through the AST and edits the tree; leaf gives you a text caret,
//! mouse, and a formatting toolbar, and turns each keystroke into an
//! offset-addressed twig edit that reparses live. You type into a document that
//! stays a valid AST the whole time.

mod ui;

use std::io::stdout;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use ratatui::{
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
            KeyEventKind, MouseEvent, MouseEventKind,
        },
        execute,
    },
    layout::Rect,
};
use leaf_core::{BlockKind, DiskState, Doc, InlineKind, InlineMarks};
use leaf_ratatui::{MouseOutcome, Outcome};

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
    /// The editor widget's own view state: horizontal/code scroll, the image
    /// raster cache, and mouse click-counting. Threaded into
    /// `leaf_ratatui::render`/`handle_key`/`handle_mouse` each frame.
    editor: leaf_ratatui::EditorState,
}

/// One row of the context menu. `Action` runs a command and closes the menu;
/// `Submenu` opens a flyout of further rows (the Format menu of styling
/// options); `Header` is a dim, unselectable section label — the divider
/// between the block and inline styles. `ui::render_context_menu` reads its
/// labels off these same values, so what's drawn and what's wired can't drift.
#[derive(Clone, Copy)]
pub enum MenuEntry {
    Action(&'static str, MenuAction),
    Submenu(&'static str, &'static [MenuEntry]),
    Header(&'static str),
}

impl MenuEntry {
    pub fn label(self) -> &'static str {
        match self {
            MenuEntry::Action(l, _) | MenuEntry::Submenu(l, _) | MenuEntry::Header(l) => l,
        }
    }

    /// A `Header` is drawn but never highlighted or activated: the arrow keys
    /// and mouse hover step over it.
    fn selectable(self) -> bool {
        !matches!(self, MenuEntry::Header(_))
    }
}

/// What activating an `Action` row does. Kept as data (rather than the old
/// `fn(&mut Doc)` pointers) so the same value can also answer "is this style
/// already on?" for the row's checkmark — see [`MenuAction::active`].
#[derive(Clone, Copy)]
pub enum MenuAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Paragraph,
    Heading(u32),
    BulletList,
    NumberedList,
    Quote,
    Inline(InlineKind),
}

impl MenuAction {
    fn run(self, doc: &mut Doc) {
        match self {
            MenuAction::Cut => clipboard_cut(doc),
            MenuAction::Copy => clipboard_copy(doc),
            MenuAction::Paste => clipboard_paste(doc),
            MenuAction::SelectAll => doc.select_all(),
            MenuAction::Paragraph => doc.set_block(BlockKind::Paragraph),
            MenuAction::Heading(n) => doc.toggle_heading(n),
            MenuAction::BulletList => doc.toggle_list(false),
            MenuAction::NumberedList => doc.toggle_list(true),
            MenuAction::Quote => doc.toggle_blockquote(),
            MenuAction::Inline(k) => doc.toggle(k),
        }
    }

    /// Whether this style is currently in force at the caret — drives the row's
    /// checkmark. Only the inline marks and headings answer cheaply and without
    /// ambiguity; the clipboard verbs and the list/quote/paragraph toggles
    /// (whose "on" state needs AST ancestry the toolbar never exposed) show none.
    pub fn active(self, marks: InlineMarks, heading: Option<u32>) -> bool {
        match self {
            MenuAction::Inline(k) => marks.contains(k),
            MenuAction::Heading(n) => heading == Some(n),
            _ => false,
        }
    }
}

/// The root right-click menu; `Format` drills into [`FORMAT_MENU`].
pub const ROOT_MENU: &[MenuEntry] = &[
    MenuEntry::Action("Cut", MenuAction::Cut),
    MenuEntry::Action("Copy", MenuAction::Copy),
    MenuEntry::Action("Paste", MenuAction::Paste),
    MenuEntry::Action("Select All", MenuAction::SelectAll),
    MenuEntry::Submenu("Format", FORMAT_MENU),
];

/// Every styling command the keyboard exposes, gathered into one flyout and
/// split into a block section (what the whole paragraph becomes) and an inline
/// section (marks on the selection). The labels are the toolbar words a reader
/// knows, not the AST kinds underneath.
pub const FORMAT_MENU: &[MenuEntry] = &[
    MenuEntry::Header("Block"),
    MenuEntry::Action("Paragraph", MenuAction::Paragraph),
    MenuEntry::Action("Heading 1", MenuAction::Heading(1)),
    MenuEntry::Action("Heading 2", MenuAction::Heading(2)),
    MenuEntry::Action("Heading 3", MenuAction::Heading(3)),
    MenuEntry::Action("Bulleted List", MenuAction::BulletList),
    MenuEntry::Action("Numbered List", MenuAction::NumberedList),
    MenuEntry::Action("Quote", MenuAction::Quote),
    MenuEntry::Header("Inline"),
    MenuEntry::Action("Bold", MenuAction::Inline(InlineKind::Strong)),
    MenuEntry::Action("Italic", MenuAction::Inline(InlineKind::Emph)),
    MenuEntry::Action("Code", MenuAction::Inline(InlineKind::Verbatim)),
    MenuEntry::Action("Highlight", MenuAction::Inline(InlineKind::Mark)),
    MenuEntry::Action("Strikethrough", MenuAction::Inline(InlineKind::Delete)),
    MenuEntry::Action("Underline", MenuAction::Inline(InlineKind::Insert)),
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
    fn new(items: &'static [MenuEntry]) -> Self {
        let selected = items.iter().position(|e| e.selectable()).unwrap_or(0);
        MenuLevel { items, selected, rect: None }
    }

    /// Move the highlight `delta` rows, skipping headers and wrapping at the
    /// ends. A no-op for a level with nothing selectable (can't happen for the
    /// two real menus, but keeps the walk total).
    fn step(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.selected as isize;
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if self.items[i as usize].selectable() {
                self.selected = i as usize;
                return;
            }
        }
    }
}

impl ContextMenu {
    fn new(anchor: (u16, u16)) -> Self {
        ContextMenu { anchor, levels: vec![MenuLevel::new(ROOT_MENU)] }
    }

    /// The frontmost (deepest) level — the one the keyboard drives.
    fn active_level(&self) -> usize {
        self.levels.len() - 1
    }

    /// Open `items` as the submenu of level `parent`, replacing any deeper level
    /// already showing (hovering a different submenu row swaps the flyout). A
    /// no-op if this exact submenu is already open, so hovering its parent row
    /// doesn't keep resetting the child's own highlight.
    fn open_submenu(&mut self, parent: usize, items: &'static [MenuEntry]) {
        if self.levels.get(parent + 1).is_some_and(|l| l.items.as_ptr() == items.as_ptr()) {
            return;
        }
        self.levels.truncate(parent + 1);
        self.levels.push(MenuLevel::new(items));
    }

    /// The `(level, row)` under a screen cell, deepest level first so a submenu
    /// wins over the parent it flies out over.
    fn hit(&self, row: u16, col: u16) -> Option<(usize, usize)> {
        for (i, level) in self.levels.iter().enumerate().rev() {
            if let Some(rect) = level.rect {
                if row >= rect.y && row < rect.y + rect.height && col >= rect.x && col < rect.x + rect.width {
                    let idx = (row - rect.y) as usize;
                    if idx < level.items.len() {
                        return Some((i, idx));
                    }
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
    loop {
        terminal.draw(|f| ui::render(f, doc, &mut app))?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(doc, key, &mut app) == Flow::Quit {
                    return Ok(());
                }
            }
            // Mouse motion (with no button down) drives the context menu's hover
            // highlight; `EnableMouseCapture` already turns on any-motion
            // reporting, so these `Moved` events arrive without extra setup. The
            // editing surface ignores them, so they cost only a redraw.
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
    // do: arrows move the highlight (skipping section headers), Right/Enter open
    // a submenu or run the highlighted row, Left/Esc back out one level (or
    // close at the root), and any other key closes it without acting.
    if let Some(menu) = &mut app.context_menu {
        let lvl = menu.active_level();
        let entry = menu.levels[lvl].items[menu.levels[lvl].selected];
        match key.code {
            KeyCode::Up => menu.levels[lvl].step(-1),
            KeyCode::Down => menu.levels[lvl].step(1),
            KeyCode::Right => {
                if let MenuEntry::Submenu(_, items) = entry {
                    menu.open_submenu(lvl, items);
                }
            }
            KeyCode::Enter => match entry {
                MenuEntry::Action(_, act) => {
                    app.context_menu = None;
                    act.run(doc);
                }
                MenuEntry::Submenu(_, items) => menu.open_submenu(lvl, items),
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
    match leaf_ratatui::handle_key(doc, key, &mut app.editor) {
        Outcome::Continue => Flow::Continue,
        // Ctrl+Q: quit, guarding an unsaved document behind the Save/Discard/
        // Cancel prompt the way it always did.
        Outcome::Quit => {
            if doc.dirty {
                app.dirty_prompt = Some(DirtyPrompt { action: DirtyAction::Quit, selected: 0 });
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
                app.dirty_prompt = Some(DirtyPrompt { action: DirtyAction::New, selected: 0 });
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
        // ⌥K / ⌥L: open a single-line prompt the host owns.
        Outcome::LinkPrompt => {
            open_link_prompt(doc, app);
            Flow::Continue
        }
        Outcome::LanguagePrompt => {
            open_language_prompt(doc, app);
            Flow::Continue
        }
    }
}

fn handle_mouse(doc: &mut Doc, m: MouseEvent, app: &mut App) {
    // The menu owns the mouse while it's open. Motion (with no button, or a
    // drag) hovers: the row under the pointer becomes the highlight, and moving
    // onto a submenu row opens its flyout while moving off it closes any deeper
    // one. A press runs the row's action (or opens its submenu); a press outside
    // every level dismisses the menu. Either way the event doesn't fall through
    // to the document underneath.
    if let Some(menu) = &mut app.context_menu {
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if let Some((lvl, idx)) = menu.hit(m.row, m.column) {
                    if menu.levels[lvl].items[idx].selectable() {
                        // Close any deeper flyout first, then highlight the row —
                        // and reopen its submenu if that's what it is.
                        menu.levels.truncate(lvl + 1);
                        menu.levels[lvl].selected = idx;
                        if let MenuEntry::Submenu(_, items) = menu.levels[lvl].items[idx] {
                            menu.open_submenu(lvl, items);
                        }
                    }
                }
            }
            MouseEventKind::Down(_) => match menu.hit(m.row, m.column) {
                Some((lvl, idx)) => match menu.levels[lvl].items[idx] {
                    MenuEntry::Action(_, act) => {
                        app.context_menu = None;
                        act.run(doc);
                    }
                    MenuEntry::Submenu(_, items) => menu.open_submenu(lvl, items),
                    MenuEntry::Header(_) => {}
                },
                None => app.context_menu = None,
            },
            _ => {}
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
            app.context_menu = Some(ContextMenu::new((x, y)));
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
    if let Ok(html) = get_clipboard_html() {
        if doc.paste_html(&html) {
            doc.status = Some("pasted".into());
            return;
        }
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
        assert_eq!(&doc.source[..7], "Title\n\n", "first ⌥1 should strip the heading marker");

        handle_key(&mut doc, alt_1, &mut app);
        assert!(doc.source.starts_with("# Title"), "second ⌥1 should re-apply H1");
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
        // A hop lands with the destination cell selected (caret at its end), so
        // typing replaces the cell — the same field-select Tab gives everywhere.
        assert_eq!(doc.selected_text(), Some("b"), "the hopped-to cell comes up selected");
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
        handle_key(&mut doc, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), &mut app);
        assert_eq!(doc.source, "| a<br> | b |\n| - | - |\n| c | d |\n");
        assert!(doc.caret_in_table(), "still editing the cell, past the break");
    }

    #[test]
    fn alt_enter_off_a_table_is_an_ordinary_newline() {
        let mut doc = doc_with("break_newline", "hello world\n");
        let mut app = App::default();
        doc.caret = 5; // after "hello"
        let breaks = doc.source.matches('\n').count();
        handle_key(&mut doc, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), &mut app);
        // Off a table `cell_line_break` declines and we fall through to the
        // ordinary newline (which opens a paragraph), so the line count grows and
        // no `<br>` is spliced.
        assert!(doc.source.matches('\n').count() > breaks, "a newline off a table");
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
        assert_eq!(get_clipboard_text().ok().as_deref(), Some("**bold**"), "the source");
        let html = get_clipboard_html().expect("html flavor");
        assert!(html.contains("<strong>bold</strong>"), "{html:?}");
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

    // ── chrome-less rendering ────────────────────────────────────────────────

    use ratatui::{Terminal, backend::TestBackend};

    /// Draw one frame of the whole UI at `w`×`h` and read the screen back as
    /// rows of text — the host chrome's own render path, exercised end to end.
    fn frame(doc: &mut Doc, app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| ui::render(f, doc, app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    #[test]
    fn the_body_starts_on_the_top_row_now_that_there_is_no_header() {
        // The document used to open under a one-row header; with the chrome gone
        // its first line is the terminal's first row.
        let mut doc = doc_with("no_header", "hello world\n");
        let mut app = App::default();
        let lines = frame(&mut doc, &mut app, 40, 6);
        assert!(lines[0].starts_with("hello world"), "body not on row 0:\n{}", lines.join("\n"));
    }

    #[test]
    fn a_dirty_prompt_floats_as_a_centered_overlay() {
        let mut doc = doc_with("prompt_overlay", "hello\n");
        let mut app = App::default();
        app.dirty_prompt = Some(DirtyPrompt { action: DirtyAction::Quit, selected: 0 });
        let lines = frame(&mut doc, &mut app, 50, 10);
        let joined = lines.join("\n");
        assert!(joined.contains("Unsaved changes"), "no dialog:\n{joined}");
        assert!(joined.contains("Save") && joined.contains("Discard"), "no choices:\n{joined}");
        // Centered, not pinned to the bottom rows the old footer used.
        let row = lines.iter().position(|l| l.contains("Unsaved changes")).unwrap();
        assert!(row > 0 && row < 9, "dialog should be centered, got row {row}");
    }

    #[test]
    fn a_status_message_shows_as_a_bottom_right_toast() {
        let mut doc = doc_with("toast", "hello\n");
        doc.status = Some("copied".into());
        let mut app = App::default();
        let lines = frame(&mut doc, &mut app, 40, 6);
        let bottom = lines.last().unwrap();
        assert!(bottom.contains("copied"), "toast missing from bottom row:\n{}", lines.join("\n"));
        // The toast is drawn flush against the right edge (its text is padded
        // with a single trailing space), and the space to its left is empty body.
        assert!(bottom.ends_with("copied "), "toast should hug the right edge: {bottom:?}");
        assert!(bottom.starts_with("     "), "toast should not stretch across the row: {bottom:?}");
    }

    #[test]
    fn a_dirty_prompt_suppresses_the_status_toast() {
        // A dialog and a toast shouldn't fight for the same glance: while the
        // dialog is up, the toast stays hidden.
        let mut doc = doc_with("no_toast_with_prompt", "hello\n");
        doc.status = Some("copied".into());
        let mut app = App::default();
        app.dirty_prompt = Some(DirtyPrompt { action: DirtyAction::Quit, selected: 0 });
        let lines = frame(&mut doc, &mut app, 40, 6);
        assert!(!lines.join("\n").contains("copied"), "toast should be suppressed:\n{}", lines.join("\n"));
    }

    // ── context menu: Format submenu, hover, active state ────────────────────

    /// Right-click, then walk the root down to the `Format` row (index 4).
    fn open_format(doc: &mut Doc, app: &mut App) {
        handle_mouse(doc, right_down(1, 2), app);
        for _ in 0..4 {
            handle_key(doc, keyp(KeyCode::Down), app);
        }
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
        assert_eq!(menu.levels[1].selected, 1);
    }

    #[test]
    fn submenu_arrows_skip_section_headers() {
        let mut doc = doc_with("submenu_headers", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        // Up from Paragraph (1) wraps past the "Inline" header (8) to the last
        // row, Underline (14) — never landing on a header.
        handle_key(&mut doc, keyp(KeyCode::Up), &mut app);
        assert_eq!(app.context_menu.as_ref().unwrap().levels[1].selected, 14);
        // Down from there wraps past the "Block" header (0) to Paragraph (1).
        handle_key(&mut doc, keyp(KeyCode::Down), &mut app);
        assert_eq!(app.context_menu.as_ref().unwrap().levels[1].selected, 1);
    }

    #[test]
    fn choosing_bold_from_the_submenu_toggles_the_selection() {
        let mut doc = doc_with("submenu_bold", "hello world\n");
        doc.anchor = Some(0);
        doc.caret = 5; // "hello" selected
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        // Paragraph(1) → Quote(7) is six Downs; a seventh skips the "Inline"
        // header to Bold(9), then Enter applies it.
        for _ in 0..7 {
            handle_key(&mut doc, keyp(KeyCode::Down), &mut app);
        }
        handle_key(&mut doc, keyp(KeyCode::Enter), &mut app);
        assert!(app.context_menu.is_none(), "running an action closes the menu");
        assert_eq!(doc.source, "**hello** world\n");
    }

    #[test]
    fn choosing_heading_from_the_submenu_sets_the_block() {
        let mut doc = doc_with("submenu_heading", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        // Paragraph(1) → Heading 1(2) is one Down.
        handle_key(&mut doc, keyp(KeyCode::Down), &mut app);
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
        let menu = app.context_menu.as_ref().expect("Left in a submenu backs out, not closes");
        assert_eq!(menu.levels.len(), 1);
        // A second Left, now at the root, closes it.
        handle_key(&mut doc, keyp(KeyCode::Left), &mut app);
        assert!(app.context_menu.is_none());
    }

    #[test]
    fn hovering_a_row_highlights_it_and_hovering_format_opens_the_submenu() {
        let mut doc = doc_with("hover", "hello\n");
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
        assert_eq!(app.context_menu.as_ref().unwrap().levels.len(), 2, "hover opens the submenu");

        // Hover back onto Cut (root row 0): the submenu closes again.
        handle_mouse(&mut doc, moved(root.y, root.x + 1), &mut app);
        assert_eq!(app.context_menu.as_ref().unwrap().levels.len(), 1, "hovering off Format closes it");
        assert_eq!(app.context_menu.as_ref().unwrap().levels[0].selected, 0);
    }

    #[test]
    fn the_format_submenu_renders_its_sections_and_flies_out() {
        let mut doc = doc_with("submenu_render", "hello\n");
        let mut app = App::default();
        open_format(&mut doc, &mut app);
        let lines = frame(&mut doc, &mut app, 60, 20);
        let joined = lines.join("\n");
        assert!(joined.contains("Format"), "the root stays visible beside the flyout:\n{joined}");
        assert!(joined.contains('▸'), "the submenu arrow is drawn:\n{joined}");
        assert!(joined.contains("Block") && joined.contains("Inline"), "section headers:\n{joined}");
        assert!(joined.contains("Bold") && joined.contains("Strikethrough"), "inline options listed:\n{joined}");
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
        assert!(joined.contains("✓ Bold"), "active Bold should be checked:\n{joined}");
        assert!(joined.contains("  Italic"), "inactive Italic should not:\n{joined}");
    }
}
