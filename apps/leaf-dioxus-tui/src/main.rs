//! leaf-dtui: the leaf-dioxus editor in a terminal.
//!
//! The Mac app's editor component, unchanged, on a Blitz document with no
//! window: a [`TermWindow`] answers its questions about size and theme, the
//! terminal's keys are handed to [`Editor::key`] as the app hands winit's,
//! clicks and the wheel go to Blitz as pointer events, and each turn the page
//! is resolved and painted into cells (blitz-tui's `paint`) instead of
//! pixels.
//!
//! The text is set in one monospace face at a size whose advance is one cell
//! wide, and a cell is that advance by one line high, so the editor's own
//! parley layout — wrapping, caret motion, hit-testing — counts in cells
//! without knowing it.
//!
//! Round the editor is unit's root, in its cell density: the context menu,
//! the command menu and the dialogs are unit's (`chrome.rs`), as the Mac
//! app's are, and every key reaches unit before the editor.
//!
//! ⌃Q quits; ⌃P or F10 opens the command menu. On macOS the editor's ⌘
//! shortcuts are ⌃ here (a terminal never sees ⌘), so ⌃S saves, ⌃B is bold,
//! ⌃Z undoes.

mod chrome;
mod commands;
mod files;
mod pictures;
mod term;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Waker};
use std::time::Duration;

use anyhow::{Context as _, Result};
use blitz_dom::DocumentConfig;
use blitz_traits::shell::{ColorScheme, Viewport};
use crossterm::event::{self, Event, MouseEventKind};
use dioxus::prelude::*;
use dioxus_core::{ScopeId, VirtualDom};
use dioxus_native::DioxusDocument;
use dioxus_native::winit::window::Window;
use leaf_core::Doc;
use leaf_dioxus::{Appearance, Editor, Grid, Host, Look, Status, Type, trace, use_shared_editor};
use unit::{Density, Keys, Theme, UnitRoot, use_unit};

use crate::chrome::{Pending, TermHost};
use crate::commands::Command;

use blitz_tui::{Metrics, PaintOptions, TermWindow};

/// The face every cell is set in, and its size in CSS pixels.
const FACE: &str = "Menlo, \"DejaVu Sans Mono\", monospace";
const SIZE: f32 = 16.0;
/// The line box over the size: one cell high.
const LEADING: f32 = 1.5;

/// Each visual line's box reaches into the column's right padding. The
/// editor breaks the lines itself and hangs a trailing space past the
/// column, as parley does; Blitz, though, broke a quote's line between its
/// bar (an inline box) and its text when the space did not fit, `white-space:
/// pre` notwithstanding — a line of the column's exact width, which cells
/// make common, came out as two rows. The room keeps it one. Not a table
/// cell's line, which the editor aligns in its cell: the room would move
/// right-aligned text off its padding.
const TERM_CSS: &str = "
.leaf .line { margin-right: -16px; }
.leaf .tcell .line { margin-right: 0; }
";

/// What the command line asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Options {
    read_only: bool,
    /// The column's width in characters; `None` fills the terminal.
    measure: Option<f32>,
}

impl Default for Options {
    /// leaf-tui's: an 80-character column, editable.
    fn default() -> Self {
        Self {
            read_only: false,
            measure: Some(80.0),
        }
    }
}

thread_local! {
    static OPTIONS: std::cell::Cell<Options> = std::cell::Cell::new(Options::default());
    static QUIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static EDITOR: RefCell<Option<Editor>> = const { RefCell::new(None) };
    static DOC: RefCell<Option<Rc<RefCell<Doc>>>> = const { RefCell::new(None) };
}

#[allow(non_snake_case)]
fn App() -> Element {
    let doc = DOC.with_borrow(|d| d.clone().expect("a document"));
    let dark = term::dark();
    let ed = use_shared_editor(
        move || doc,
        move || Look {
            ty: Type {
                body: FACE.into(),
                mono: FACE.into(),
                size: SIZE,
                leading: LEADING,
                grid: Some(grid()),
                faces: Default::default(),
            },
            measure: OPTIONS.get().measure,
            // A terminal's cells have no room for a bar a few pixels wide.
            scroll_bar: false,
            appearance: if dark {
                Appearance::Dark
            } else {
                Appearance::Light
            },
            status: Status::Full,
        },
        || Rc::new(TermHost) as Rc<dyn Host>,
    );
    use_hook({
        let ed = ed.clone();
        move || EDITOR.with_borrow_mut(|e| *e = Some(ed))
    });
    // unit in the editor's face, its page background left to the editor's.
    let mut theme = Theme {
        font: FACE,
        heading_font: FACE,
        ..Theme::default()
    };
    theme.light.bg = "transparent";
    theme.dark.bg = "transparent";
    let appearance = if dark {
        unit::Appearance::Dark
    } else {
        unit::Appearance::Light
    };
    rsx! {
        UnitRoot { keys: Keys::Host, density: Density::Cell, appearance, theme,
            KeepUnit {}
            {ed.render()}
            style { {TERM_CSS} }
        }
    }
}

/// Hands the page's unit to the chrome, which gives it every key first.
#[allow(non_snake_case)]
fn KeepUnit() -> Element {
    let unit = use_unit();
    use_hook(|| chrome::set_unit(unit));
    rsx! {}
}

/// One cell of the editor's face.
fn cell() -> Metrics {
    Metrics::measure(FACE, SIZE, LEADING)
}

/// The cell, as the editor sizes its chrome and tables in it.
fn grid() -> Grid {
    let m = cell();
    Grid {
        width: m.cw,
        height: m.ch,
    }
}

/// How the page is painted: a cell of the editor's face, and the carets
/// that are the terminal's cursor — the editor's `.caret-in` and unit's
/// `.unit-caret` in a dialog's field.
fn paint_options() -> PaintOptions {
    let mut opts = PaintOptions::new(cell());
    opts.cursor_classes = vec!["caret-in".into(), "unit-caret".into()];
    opts
}

struct Session {
    doc: DioxusDocument,
    window: Arc<TermWindow>,
    watch: RefCell<files::Watch>,
    look: PaintOptions,
    m: Metrics,
    cols: u16,
    rows: u16,
}

impl Session {
    fn new(path: PathBuf, cols: u16, rows: u16) -> Result<Self> {
        enter_runtime();
        let mut source = Doc::open_or_create(path.clone())
            .with_context(|| format!("opening {}", path.display()))?;
        if OPTIONS.get().read_only {
            source.set_read_only(true);
            source.status = Some("read-only".into());
        }
        let watch = RefCell::new(files::Watch::new(&source));
        DOC.with_borrow_mut(|d| *d = Some(Rc::new(RefCell::new(source))));
        let look = paint_options();
        let m = look.metrics;
        let (w, h) = m.px(cols, rows);
        let window = Arc::new(TermWindow::new(w, h, term::dark()));
        let as_dyn: Arc<dyn Window> = window.clone();
        let mut doc = DioxusDocument::new(VirtualDom::new(App), DocumentConfig::default());
        doc.vdom
            .in_scope(ScopeId::ROOT, move || provide_context(as_dyn));
        doc.inner.borrow_mut().set_viewport(viewport(w, h));
        doc.initial_build();
        if let Some(unit) = chrome::unit() {
            doc.vdom
                .in_scope(ScopeId::ROOT, || unit.set_viewport(w as f64, h as f64));
        }
        let mut s = Self {
            doc,
            window,
            watch,
            look,
            m,
            cols,
            rows,
        };
        s.settle();
        Ok(s)
    }

    /// Poll the virtual DOM and lay the page out until it holds still:
    /// whether anything changed — a timer's work, such as the word count,
    /// changes the page with no input to show for it.
    fn settle(&mut self) -> bool {
        let mut any = false;
        for _ in 0..3 {
            let changed =
                blitz_dom::Document::poll(&mut self.doc, Some(Context::from_waker(Waker::noop())));
            self.doc.inner.borrow_mut().resolve(0.0);
            any |= changed;
            if !changed {
                break;
            }
        }
        any
    }

    fn with_editor(&self, f: impl FnOnce(&Editor)) {
        let ed = EDITOR.with_borrow(|e| e.clone());
        if let Some(ed) = ed {
            self.doc.vdom.in_scope(ScopeId::ROOT, || f(&ed));
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        let (w, h) = self.m.px(cols, rows);
        self.window.resize(w, h);
        self.doc.inner.borrow_mut().set_viewport(viewport(w, h));
        self.with_editor(|ed| ed.resize(w, h));
        self.with_unit(|u| u.set_viewport(w as f64, h as f64));
    }

    fn with_unit<T>(&self, f: impl FnOnce(&unit::Unit) -> T) -> Option<T> {
        let unit = chrome::unit()?;
        Some(self.doc.vdom.in_scope(ScopeId::ROOT, || f(&unit)))
    }

    /// A key, to unit first — an open menu or dialog takes it, and a dialog's
    /// field takes typed text — then the command menu and Quit, then the
    /// editor.
    fn key(&self, key: dioxus::html::Key, code: dioxus::html::Code, mods: dioxus::html::Modifiers) {
        use dioxus::html::{Key, Modifiers, NamedKey};
        let typed = match &key {
            Key::Character(t) if !mods.intersects(Modifiers::META | Modifiers::CONTROL) => {
                Some(t.clone())
            }
            _ => None,
        };
        trace(|| format!("host key {key:?} {code:?} {mods:?}"));
        let taken = self
            .with_unit(|u| match &typed {
                Some(t) if u.is_open() => u.text(t),
                _ => u.key(&key, mods),
            })
            .unwrap_or(false);
        if taken {
            return;
        }
        let primary = mods.contains(term::PRIMARY);
        let letter = |c: &str| matches!(&key, Key::Character(k) if k.eq_ignore_ascii_case(c));
        if matches!(key, Key::Named(NamedKey::F10)) || (primary && letter("p")) {
            self.with_unit(chrome::command_menu);
        } else if primary && letter("q") {
            self.run_pending(Pending::Command(Command::Quit));
        } else {
            self.with_editor(|ed| ed.key(key, code, mods));
        }
    }

    /// Carry out a choice; `true` to quit.
    fn run_pending(&self, p: Pending) -> bool {
        let mut quit = false;
        match p {
            Pending::Command(Command::Chord(k, c, m)) => self.with_editor(|ed| ed.key(k, c, m)),
            Pending::Command(Command::Link) => {
                let current = editor()
                    .and_then(|ed| ed.doc().link_destination_at_caret())
                    .unwrap_or_default();
                self.with_unit(|u| chrome::prompt_link(u, current));
            }
            Pending::Command(Command::Quit) => {
                let (dirty, name) = editor()
                    .map(|ed| {
                        let d = ed.doc();
                        (d.dirty, d.file_name())
                    })
                    .unwrap_or((false, String::new()));
                if dirty {
                    self.with_unit(|u| chrome::confirm_quit(u, &name));
                } else {
                    quit = true;
                }
            }
            Pending::FollowFootnote => self.with_editor(|ed| {
                ed.follow_footnote();
            }),
            Pending::OpenLink => self.with_editor(|ed| {
                ed.open_link();
            }),
            Pending::CopyLink => self.with_editor(|ed| ed.copy_link()),
            Pending::InsertLink(dest) => self.with_editor(|ed| {
                ed.doc().insert_link(&dest);
                ed.refresh();
            }),
            Pending::Save => {
                self.save();
            }
            Pending::Command(Command::SaveAs) => {
                let current = editor().map(|ed| ed.doc().path.display().to_string());
                self.with_unit(|u| files::prompt_save_as(u, current.unwrap_or_default()));
            }
            Pending::SaveTo(path) => self.with_editor(|ed| {
                ed.doc().save_as(path);
                self.watch.borrow_mut().saw_save(&ed.doc());
                ed.refresh();
            }),
            Pending::Overwrite => self.with_editor(|ed| {
                ed.doc().save();
                self.watch.borrow_mut().saw_save(&ed.doc());
                ed.refresh();
            }),
            Pending::Reload => self.with_editor(|ed| {
                ed.doc().reload();
                self.watch.borrow_mut().saw_save(&ed.doc());
                ed.refresh();
            }),
            // Quit only once the save has gone through without a question;
            // one that asks (where to, or whether to overwrite) stays.
            Pending::SaveAndQuit => {
                quit = self.save() && !editor().is_some_and(|ed| ed.doc().dirty)
            }
            Pending::QuitNow => quit = true,
        }
        if quit {
            QUIT.set(true);
        }
        quit
    }

    /// Save, or ask what the save needs first: whether it wrote.
    fn save(&self) -> bool {
        let Some(ed) = editor() else { return false };
        let plan = files::plan(&ed.doc());
        match plan {
            files::Save::Write => {
                self.with_editor(|ed| {
                    ed.doc().save();
                    self.watch.borrow_mut().saw_save(&ed.doc());
                    ed.refresh();
                });
                return true;
            }
            files::Save::AskWhere => {
                self.with_unit(|u| files::prompt_save_as(u, String::new()));
            }
            files::Save::AskOverwrite => {
                let name = ed.doc().file_name();
                self.with_unit(|u| files::confirm_overwrite(u, &name));
            }
            files::Save::ReadOnly => self.with_editor(|ed| {
                ed.doc().status = Some("read-only".into());
                ed.refresh();
            }),
        }
        false
    }

    /// One look at the file, when it is time: whether the page changed.
    fn tick_watch(&self) -> bool {
        let mut changed = false;
        self.with_editor(|ed| changed = self.watch.borrow_mut().tick(ed));
        changed
    }

    fn paint(&self) -> blitz_tui::Grid {
        blitz_tui::paint(&self.doc.inner.borrow(), &self.look, self.cols, self.rows)
    }

    fn draw(&self, out: &mut blitz_tui::Terminal, grid: &blitz_tui::Grid) -> Result<()> {
        out.draw(grid, &self.doc.inner.borrow())?;
        Ok(())
    }
}

/// The editor's timers are tokio's: a runtime on a thread of its own keeps
/// them firing while this one waits on the terminal. Entered once per thread
/// and left entered.
fn enter_runtime() {
    use std::sync::OnceLock;
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    thread_local! { static ENTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if ENTERED.get() {
        return;
    }
    let rt = RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
            .expect("a tokio runtime")
    });
    std::mem::forget(rt.enter());
    ENTERED.set(true);
}

fn editor() -> Option<Editor> {
    EDITOR.with_borrow(|e| e.clone())
}

fn viewport(w: u32, h: u32) -> Viewport {
    let scheme = if term::dark() {
        ColorScheme::Dark
    } else {
        ColorScheme::Light
    };
    Viewport::new(w, h, 1.0, scheme)
}

fn main() -> Result<()> {
    let mut args: Vec<_> = std::env::args_os().skip(1).collect();
    // leaf-tui's flags: `-r` reads only, `-w N` sets the column to N
    // characters (0 fills the terminal).
    let mut opts = Options::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].to_str() {
            Some("-r" | "--read-only") => {
                opts.read_only = true;
                args.remove(i);
            }
            Some("-w" | "--width") if i + 1 < args.len() => {
                let n: f32 = args[i + 1]
                    .to_str()
                    .and_then(|n| n.parse().ok())
                    .context("-w takes a number of characters")?;
                opts.measure = (n > 0.0).then_some(n);
                args.drain(i..i + 2);
            }
            _ => i += 1,
        }
    }
    OPTIONS.set(opts);
    // `--dump`: one frame, 80 × 24, printed as text — what a test or a
    // reader without a terminal to hand can look at. `--dump-html`: the same
    // frame as a `<pre>`, in its colours.
    let dump = args
        .first()
        .is_some_and(|a| a == "--dump" || a == "--dump-html");
    let html = args.first().is_some_and(|a| a == "--dump-html");
    if dump {
        args.remove(0);
    }
    let path = args
        .first()
        .map(PathBuf::from)
        .context("usage: leaf-dtui [--dump | --dump-html] [-r] [-w N] <file>")?;

    if dump {
        let mut s = Session::new(path, 80, 24)?;
        // Past the word count's settling time, so the status line is whole.
        std::thread::sleep(Duration::from_millis(500));
        s.settle();
        if std::env::var_os("LEAF_DTUI_HTML").is_some() {
            let doc = s.doc.inner.borrow();
            eprintln!("{}", doc.root_element().outer_html_pretty());
        }
        let grid = s.paint();
        if html {
            print!("{}", grid.html());
        } else {
            print!("{}", grid.text());
        }
        return Ok(());
    }
    // Anything written to stderr would land on the screen among the cells.
    quiet_stderr();
    term::ask_theme();
    let (cols, rows) = crossterm::terminal::size()?;
    let mut s = Session::new(path, cols, rows)?;
    let mut out = term::enter()?;
    let result = run(&mut s, &mut out);
    out.leave()?;
    result
}

/// Send stderr to `LEAF_DTUI_LOG`, or nowhere.
fn quiet_stderr() {
    use std::os::fd::AsRawFd;
    let path = std::env::var_os("LEAF_DTUI_LOG").unwrap_or_else(|| "/dev/null".into());
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // SAFETY: both descriptors are open; stderr is replaced, not closed.
        unsafe { libc::dup2(f.as_raw_fd(), 2) };
    }
}

fn run(s: &mut Session, out: &mut blitz_tui::Terminal) -> Result<()> {
    let mut grid = s.paint();
    s.draw(out, &grid)?;
    loop {
        let mut acted = false;
        if event::poll(Duration::from_millis(50))? {
            while {
                match event::read()? {
                    Event::Key(k) => {
                        if let Some((key, code, mods)) = term::key(&k) {
                            s.key(key, code, mods);
                        }
                    }
                    Event::Paste(text) => {
                        if !s
                            .with_unit(|u| u.is_open() && u.text(&text))
                            .unwrap_or(false)
                        {
                            for c in text.chars() {
                                let (key, code, mods) = term::char_key(c);
                                s.with_editor(|ed| ed.key(key, code, mods));
                            }
                        }
                    }
                    Event::Mouse(me) => match me.kind {
                        MouseEventKind::ScrollDown => {
                            s.with_editor(|ed| ed.scroll(-3.0 * s.m.ch as f64))
                        }
                        MouseEventKind::ScrollUp => {
                            s.with_editor(|ed| ed.scroll(3.0 * s.m.ch as f64))
                        }
                        _ => {
                            if let MouseEventKind::Down(_) = me.kind {
                                chrome::set_pointer((
                                    me.column as f64 * s.m.cw as f64,
                                    me.row as f64 * s.m.ch as f64,
                                ));
                            }
                            term::pointer(&mut s.doc, s.m, &me);
                        }
                    },
                    // Asked below, every turn.
                    Event::Resize(..) => {}
                    _ => {}
                }
                event::poll(Duration::ZERO)?
            } {}
            acted = true;
        }
        // The terminal's size, asked each turn rather than trusted to its
        // resize events: one that arrives while the start-up queries read
        // stdin is lost, and a window that grows just after it opens (a new
        // terminal window settling) would keep the old frame.
        let (c, r) = crossterm::terminal::size()?;
        if (c, r) != (s.cols, s.rows) {
            s.resize(c, r);
            out.clear()?;
            acted = true;
        }
        if s.tick_watch() {
            acted = true;
        }
        let t0 = std::time::Instant::now();
        if s.settle() {
            acted = true;
        }
        // What the menus and dialogs chose, now the events are over.
        let chosen = chrome::take();
        if !chosen.is_empty() {
            for p in chosen {
                s.run_pending(p);
            }
            s.settle();
            acted = true;
        }
        if QUIT.get() {
            return Ok(());
        }
        if acted || s.window.take_dirty() {
            let t1 = std::time::Instant::now();
            let next = s.paint();
            trace(|| format!("frame: settle {:.1?} paint {:.1?}", t1 - t0, t1.elapsed()));
            if next != grid {
                s.draw(out, &next)?;
                grid = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of the test's own, holding `doc.md` with `source`.
    fn scratch(source: &str) -> (PathBuf, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("agent-leaf-dtui-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, source).unwrap();
        (dir, path)
    }

    /// A frame of `source`, 80 × 24, as text.
    fn frame(source: &str) -> String {
        let (dir, path) = scratch(source);
        let s = Session::new(path, 80, 24).unwrap();
        let text = s.paint().text();
        std::fs::remove_dir_all(&dir).ok();
        text
    }

    #[test]
    fn a_heading_set_larger_than_a_cell_reads_as_one_word() {
        let f = frame("# Title\n\nBody.\n");
        // The caret sits before the heading's first letter.
        assert!(f.lines().any(|l| l.trim() == "▏Title"), "{f}");
        assert!(f.lines().any(|l| l.trim() == "Body."), "{f}");
    }

    #[test]
    fn a_list_item_draws_its_text_once() {
        let f = frame("Top.\n\n- one\n- two\n  - nested\n");
        assert!(f.lines().any(|l| l.trim() == "• one"), "{f}");
        assert!(f.lines().any(|l| l.trim() == "◦ nested"), "{f}");
    }

    #[test]
    fn a_table_keeps_its_text_over_its_borders() {
        let f = frame("Top.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        let a = f.lines().find(|l| l.contains('a')).unwrap();
        assert!(a.contains('b'), "{f}");
        assert!(f.lines().any(|l| l.contains('1') && l.contains('2')), "{f}");
    }

    #[test]
    fn a_table_is_ruled_as_leaf_tui_rules_it() {
        let f = frame("Top.\n\n| Name | Qty |\n|---|---:|\n| Pear | 3 |\n| Fig | 12 |\n");
        let table: Vec<&str> = f
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with(['┌', '│', '├', '└']))
            .collect();
        assert_eq!(
            table,
            [
                "┌──────┬─────┐",
                "│ Name │ Qty │",
                "├──────┼─────┤",
                "│ Pear │   3 │",
                "│ Fig  │  12 │",
                "└──────┴─────┘",
            ],
            "{f}"
        );
    }

    #[test]
    fn a_click_in_a_table_cell_lands_on_the_character_under_it() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent};
        let (dir, path) = scratch("Top.\n\n| Name | Qty |\n|---|---:|\n| Fig | 12 |\n");
        let mut s = Session::new(path, 80, 24).unwrap();
        let f = s.paint().text();
        let (row, line) = f
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains("12"))
            .unwrap();
        // The cell of the `2`.
        let column = line[..line.find("12").unwrap()].chars().count() as u16 + 1;
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            let me = MouseEvent {
                kind,
                column,
                row: row as u16,
                modifiers: KeyModifiers::NONE,
            };
            term::pointer(&mut s.doc, s.m, &me);
        }
        s.settle();
        let (key, code, mods) = term::char_key('x');
        s.with_editor(|ed| ed.key(key, code, mods));
        s.settle();
        let f = s.paint().text();
        std::fs::remove_dir_all(&dir).ok();
        assert!(f.contains("1x▏2"), "{f}");
    }

    #[test]
    fn the_find_bar_is_one_row_and_replace_another() {
        let (dir, path) = scratch("Text.\n");
        let mut s = Session::new(path, 80, 24).unwrap();
        let rows = |s: &Session| {
            let mut h = 0.0;
            s.with_editor(|ed| h = ed.bar_height());
            h / s.m.ch as f64
        };
        let (key, code, _) = term::char_key('f');
        s.key(key.clone(), code, term::PRIMARY);
        s.settle();
        let find = rows(&s);
        let f = s.paint().text();
        s.key(key, code, term::PRIMARY | dioxus::html::Modifiers::ALT);
        s.settle();
        let replace = rows(&s);
        let g = s.paint().text();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!((find, replace), (1.0, 2.0));
        assert!(
            f.lines().next().unwrap().trim_start().starts_with("Find"),
            "{f}"
        );
        assert!(
            g.lines()
                .nth(1)
                .unwrap()
                .trim_start()
                .starts_with("Replace"),
            "{g}"
        );
        assert!(g.lines().nth(2).unwrap().contains("Text."), "{g}");
    }

    #[test]
    fn the_status_line_is_the_last_row() {
        let f = frame("Text.\n");
        let last = f.lines().last().unwrap();
        assert!(
            last.contains("doc.md") && last.contains("wysiwyg view"),
            "{f}"
        );
    }

    #[test]
    fn a_typed_key_lands_at_the_caret() {
        let (dir, path) = scratch("Text.\n");
        let mut s = Session::new(path, 80, 24).unwrap();
        for c in "Hi ".chars() {
            let (key, code, mods) = term::char_key(c);
            s.with_editor(|ed| ed.key(key, code, mods));
        }
        s.settle();
        let f = s.paint().text();
        std::fs::remove_dir_all(&dir).ok();
        assert!(f.lines().any(|l| l.trim() == "Hi ▏Text."), "{f}");
    }

    #[test]
    fn a_picture_is_recorded_and_the_status_line_covers_its_last_row() {
        let png = concat!(env!("CARGO_MANIFEST_DIR"), "/../../leaf-gpui.png");
        let (dir, path) = scratch(&format!("Top.\n\n![shot]({png})\n"));
        let s = Session::new(path, 80, 12).unwrap();
        let g = s.paint();
        std::fs::remove_dir_all(&dir).ok();
        let [p] = &g.pictures[..] else {
            panic!("one picture: {:?}", g.pictures)
        };
        assert!(p.src.starts_with("file://"), "{p:?}");
        // Taller than the screen: shown from its top down to the status
        // line, the last row.
        assert!(p.row + p.rows as i32 > 12, "{p:?}");
        assert_eq!(p.shown, (p.row, 11), "{p:?}");
    }

    #[test]
    fn the_command_menu_opens_over_the_page() {
        let (dir, path) = scratch("Text.\n");
        let mut s = Session::new(path, 80, 24).unwrap();
        let (key, code, _) = term::char_key('p');
        s.key(key, code, term::PRIMARY);
        s.settle();
        let open = s.with_unit(|u| u.is_open()).unwrap();
        let f = s.paint().text();
        std::fs::remove_dir_all(&dir).ok();
        assert!(open, "the menu opened");
        assert!(f.contains("File") && f.contains("Paragraph"), "{f}");
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_and_the_keys_answer() {
        use dioxus::html::{Code, Key, Modifiers, NamedKey};
        let (dir, path) = scratch("Text.\n");
        let mut s = Session::new(path, 80, 24).unwrap();
        let (key, code, mods) = term::char_key('x');
        s.key(key, code, mods);
        s.settle();
        let (key, code, _) = term::char_key('q');
        s.key(key, code, term::PRIMARY);
        s.settle();
        assert!(s.with_unit(|u| u.is_open()).unwrap(), "the sheet is open");
        assert!(s.paint().text().contains("Save the changes to doc.md?"));
        let down = || {
            (
                Key::Named(NamedKey::ArrowDown),
                Code::ArrowDown,
                Modifiers::empty(),
            )
        };
        for (k, c, m) in [down(), down()] {
            s.key(k, c, m);
        }
        s.key(Key::Named(NamedKey::Enter), Code::Enter, Modifiers::empty());
        s.settle();
        let chosen = chrome::take();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(chosen, vec![Pending::QuitNow]);
    }

    #[test]
    fn a_clean_document_takes_an_outside_write_once_it_stands_still() {
        let (dir, path) = scratch("First.\n");
        let mut s = Session::new(path.clone(), 80, 24).unwrap();
        std::fs::write(&path, "First.\n\nWritten outside.\n").unwrap();
        // The look that notices waits; the next takes it.
        for _ in 0..2 {
            std::thread::sleep(files::POLL + Duration::from_millis(50));
            s.tick_watch();
        }
        s.settle();
        let f = s.paint().text();
        std::fs::remove_dir_all(&dir).ok();
        assert!(f.contains("Written outside."), "{f}");
        assert!(f.contains("reloaded from disk"), "{f}");
    }

    #[test]
    fn a_menu_opened_over_a_picture_is_not_painted_over() {
        let png = concat!(env!("CARGO_MANIFEST_DIR"), "/../../leaf-gpui.png");
        let (dir, path) = scratch(&format!("Top.\n\n![shot]({png})\n"));
        let mut s = Session::new(path, 80, 24).unwrap();
        let (key, code, _) = term::char_key('p');
        s.key(key, code, term::PRIMARY);
        s.settle();
        let g = s.paint();
        std::fs::remove_dir_all(&dir).ok();
        let [p] = &g.pictures[..] else {
            panic!("one picture: {:?}", g.pictures)
        };
        // Every cell of the menu's labels lies outside what the picture
        // still paints.
        let inside = |c: i32, r: i32| {
            (p.shown_cols.0..p.shown_cols.1).contains(&c) && (p.shown.0..p.shown.1).contains(&r)
        };
        let mut seen = 0;
        for r in 0..g.rows {
            let line: String = (0..g.cols).map(|c| g.get(c, r).text.clone()).collect();
            for label in ["File", "Edit", "Paragraph"] {
                if let Some(at) = line.find(label) {
                    seen += 1;
                    let c = line[..at].chars().count() as i32;
                    assert!(!inside(c, r as i32), "{label} at {c},{r} under {p:?}");
                }
            }
        }
        assert_eq!(seen, 3, "{}", g.text());
        assert!(
            p.shown.0 < p.shown.1,
            "some of the picture still shows: {p:?}"
        );
    }
}
