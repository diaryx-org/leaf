//! leaf-dtui: the leaf-dioxus editor in a terminal.
//!
//! The Mac app's editor component, unchanged, on a Blitz document with no
//! window: a [`TermWindow`] answers its questions about size and theme, the
//! terminal's keys are handed to [`Editor::key`] as the app hands winit's,
//! clicks and the wheel go to Blitz as pointer events, and each turn the page
//! is resolved and painted into cells ([`paint`]) instead of pixels.
//!
//! The text is set in one monospace face at a size whose advance is one cell
//! wide, and a cell is that advance by one line high, so the editor's own
//! parley layout — wrapping, caret motion, hit-testing — counts in cells
//! without knowing it.
//!
//! ⌃Q quits. On macOS the editor's ⌘ shortcuts are ⌃ here (a terminal never
//! sees ⌘), so ⌃S saves, ⌃B is bold, ⌃Z undoes.

mod paint;
mod term;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Waker};
use std::time::Duration;

use anyhow::{Context as _, Result};
use blitz_dom::DocumentConfig;
use blitz_traits::shell::{ColorScheme, Viewport};
use crossterm::event::{self, Event, MouseButton, MouseEventKind};
use dioxus::prelude::*;
use dioxus_core::{ScopeId, VirtualDom};
use dioxus_native::DioxusDocument;
use dioxus_native::winit::window::Window;
use leaf_core::Doc;
use leaf_dioxus::{Appearance, Editor, Host, Look, Request, Status, Type, use_shared_editor};

use crate::paint::Metrics;
use crate::window::TermWindow;

/// The face every cell is set in, and its size in CSS pixels.
const FACE: &str = "Menlo, \"DejaVu Sans Mono\", monospace";
const SIZE: f32 = 16.0;
/// The line box over the size: one cell high. 1.5, the editor's own
/// leading, makes a cell 24px — the height of its status line, which is then
/// exactly a row.
const LEADING: f32 = 1.5;

/// Pixel chrome the editor sizes for a window, recut to whole cells.
const TERM_CSS: &str = "
.status { font: 16px Menlo, monospace; border-top: 0; }
.find { font: 16px Menlo, monospace; }
";

thread_local! {
    static EDITOR: RefCell<Option<Editor>> = const { RefCell::new(None) };
    static DOC: RefCell<Option<Rc<RefCell<Doc>>>> = const { RefCell::new(None) };
}

struct TermHost;

impl Host for TermHost {
    fn request(&self, ed: &Editor, request: Request) {
        if let Request::Save = request {
            ed.doc().save();
            ed.refresh();
        }
    }
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
            },
            measure: None,
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
    rsx! {
        {ed.render()}
        style { {TERM_CSS} }
    }
}

/// One cell's size: the face's advance, and the line box.
fn metrics() -> Metrics {
    use parley::{FontContext, LayoutContext, StyleProperty};
    let mut fcx = FontContext::default();
    let mut lcx: LayoutContext<()> = LayoutContext::new();
    let probe = "0".repeat(100);
    let mut b = lcx.ranged_builder(&mut fcx, &probe, 1.0, true);
    b.push_default(StyleProperty::FontFamily(parley::FontFamily::Source(
        FACE.into(),
    )));
    b.push_default(StyleProperty::FontSize(SIZE));
    let mut layout = b.build(&probe);
    layout.break_all_lines(None);
    Metrics {
        cw: layout.width() / 100.0,
        ch: SIZE * LEADING,
    }
}

struct Session {
    doc: DioxusDocument,
    window: Arc<TermWindow>,
    m: Metrics,
    cols: u16,
    rows: u16,
}

impl Session {
    fn new(path: PathBuf, cols: u16, rows: u16) -> Result<Self> {
        enter_runtime();
        let source = Doc::open_or_create(path.clone())
            .with_context(|| format!("opening {}", path.display()))?;
        DOC.with_borrow_mut(|d| *d = Some(Rc::new(RefCell::new(source))));
        let m = metrics();
        let (w, h) = px(m, cols, rows);
        let window = Arc::new(TermWindow::new(w, h, term::dark()));
        let as_dyn: Arc<dyn Window> = window.clone();
        let mut doc = DioxusDocument::new(VirtualDom::new(App), DocumentConfig::default());
        doc.vdom
            .in_scope(ScopeId::ROOT, move || provide_context(as_dyn));
        doc.inner.borrow_mut().set_viewport(viewport(w, h));
        doc.initial_build();
        let mut s = Self {
            doc,
            window,
            m,
            cols,
            rows,
        };
        s.settle();
        Ok(s)
    }

    /// Poll the virtual DOM and lay the page out until it holds still.
    fn settle(&mut self) {
        for _ in 0..3 {
            let changed =
                blitz_dom::Document::poll(&mut self.doc, Some(Context::from_waker(Waker::noop())));
            self.doc.inner.borrow_mut().resolve(0.0);
            if !changed {
                break;
            }
        }
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
        let (w, h) = px(self.m, cols, rows);
        self.window.resize(w, h);
        self.doc.inner.borrow_mut().set_viewport(viewport(w, h));
        self.with_editor(|ed| ed.resize(w, h));
    }

    fn paint(&self) -> paint::Grid {
        paint::paint(&self.doc.inner.borrow(), self.m, self.cols, self.rows)
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

fn px(m: Metrics, cols: u16, rows: u16) -> (u32, u32) {
    (
        (cols as f32 * m.cw).round() as u32,
        (rows as f32 * m.ch).round() as u32,
    )
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
        .context("usage: leaf-dtui [--dump | --dump-html] <file>")?;

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
    let (cols, rows) = crossterm::terminal::size()?;
    let mut s = Session::new(path, cols, rows)?;
    let mut out = term::Terminal::enter()?;
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

fn run(s: &mut Session, out: &mut term::Terminal) -> Result<()> {
    let mut grid = s.paint();
    out.draw(&grid)?;
    loop {
        let mut acted = false;
        if event::poll(Duration::from_millis(50))? {
            while {
                match event::read()? {
                    Event::Key(k) if term::is_quit(&k) => return Ok(()),
                    Event::Key(k) => {
                        if let Some((key, code, mods)) = term::key(&k) {
                            s.with_editor(|ed| ed.key(key, code, mods));
                        }
                    }
                    Event::Paste(text) => {
                        for c in text.chars() {
                            let (key, code, mods) = term::char_key(c);
                            s.with_editor(|ed| ed.key(key, code, mods));
                        }
                    }
                    Event::Mouse(me) => match me.kind {
                        MouseEventKind::ScrollDown => {
                            s.with_editor(|ed| ed.scroll(-3.0 * s.m.ch as f64))
                        }
                        MouseEventKind::ScrollUp => {
                            s.with_editor(|ed| ed.scroll(3.0 * s.m.ch as f64))
                        }
                        MouseEventKind::Down(MouseButton::Left)
                        | MouseEventKind::Up(MouseButton::Left)
                        | MouseEventKind::Drag(MouseButton::Left) => {
                            term::pointer(&mut s.doc, s.m, &me);
                        }
                        _ => {}
                    },
                    Event::Resize(c, r) => s.resize(c, r),
                    _ => {}
                }
                event::poll(Duration::ZERO)?
            } {}
            acted = true;
        }
        let t0 = std::time::Instant::now();
        s.settle();
        if acted || s.window.take_dirty() {
            let t1 = std::time::Instant::now();
            let next = s.paint();
            leaf_dioxus::trace(|| format!("frame: settle {:.1?} paint {:.1?}", t1 - t0, t1.elapsed()));
            if next != grid {
                out.draw_diff(&grid, &next)?;
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
}
