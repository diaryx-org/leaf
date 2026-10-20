//! A window's page — a document window's toolbar and editor, or the Settings
//! window — and the [`Page`] the app keeps for each, through which the
//! window's events and the menus' commands reach the editor.
//!
//! A document's page is the toolbar (`toolbar.rs`) over the editor, whose
//! box ([`leaf_dioxus::Bounds`]) starts below it, all under unit's root
//! ([`unit::UnitRoot`]), whose overlay is over both for its menus and
//! dialogs. Where
//! the platform's title bar is transparent over the page, the toolbar's
//! band runs up behind it (`platform::TITLE_BAR`).
//!
//! What the menus and the toolbar show — the marks in force, the heading,
//! whether there is anything to undo — is the [`Chrome`], read off the
//! document after every turn that repainted it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dioxus::prelude::*;
use dioxus_core::{Runtime, ScopeId};
use dioxus_native::winit::event::{ElementState, Ime, WindowEvent};
use dioxus_native::winit::keyboard::ModifiersState;
use dioxus_native::winit::window::Window;
use leaf_core::{Doc, InlineMarks, LineFlow, MarkColor, View};
use leaf_dioxus::{Bounds, Editor, Host, pressed, themed, use_shared_editor};
use unit::{Keys, Theme, Unit, UnitRoot, use_unit};

use crate::app::{self, AppEvent};
use crate::commands::Command;
use crate::platform;
use crate::shell::Shell;

/// The toolbar's row, under the title bar's band.
pub const TOOLBAR: f64 = 38.0;

/// Where a document's editor starts: below the title bar's band and the
/// toolbar.
pub fn chrome_height() -> f64 {
    platform::TITLE_BAR + TOOLBAR
}

/// What the menus and the toolbar read off a document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chrome {
    pub revision: u64,
    pub dirty: bool,
    pub source_view: bool,
    pub in_table: bool,
    pub has_selection: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// The caret's heading level, 0 in a paragraph.
    pub heading: u32,
    pub code_block: bool,
    pub verse: bool,
    pub blockquote: bool,
    /// The caret's task: none, open, done.
    pub task: Option<bool>,
    pub marks: InlineMarks,
    pub link: bool,
    pub mark_color: Option<MarkColor>,
    pub preserve_breaks: bool,
}

impl Chrome {
    pub fn of(d: &mut Doc) -> Self {
        Self {
            revision: d.revision(),
            dirty: d.dirty,
            source_view: d.view == View::Source,
            in_table: d.caret_in_table(),
            has_selection: d.selection().is_some(),
            can_undo: d.can_undo(),
            can_redo: d.can_redo(),
            heading: d.current_heading_level().unwrap_or(0),
            code_block: d.caret_in_code_block(),
            verse: d.caret_in_verse(),
            blockquote: d.caret_in_blockquote(),
            task: d.task_checked_at_caret(),
            marks: d.active_inline_marks(),
            link: d.link_destination_at_caret().is_some(),
            mark_color: d.mark_color_at_caret(),
            preserve_breaks: d.line_flow() == LineFlow::Preserve,
        }
    }
}

/// What a page leaves behind for the app once it has made them: its
/// [`Unit`], and for a document its editor and the signal its toolbar reads.
#[derive(Clone, Default)]
pub struct Made {
    pub unit: Rc<Cell<Option<Unit>>>,
    pub editor: Rc<RefCell<Option<Editor>>>,
    pub chrome: Rc<Cell<Option<Signal<Chrome>>>>,
}

/// What a document's page is made with.
#[derive(Clone)]
pub struct DocInit {
    pub doc: Rc<RefCell<Doc>>,
    pub shell: Rc<Shell>,
    pub made: Made,
}

impl DocInit {
    pub fn new(doc: Doc, shell: Shell) -> Self {
        Self {
            doc: Rc::new(RefCell::new(doc)),
            shell: Rc::new(shell),
            made: Made::default(),
        }
    }
}

/// A document window's page: the toolbar over the editor, and the overlay
/// over both.
#[allow(non_snake_case)]
pub fn DocumentPage() -> Element {
    let init = use_context::<DocInit>();
    let chrome = use_signal(Chrome::default);
    let ed = use_shared_editor(
        || init.doc.clone(),
        || init.shell.look(),
        || init.shell.clone() as Rc<dyn Host>,
    );
    use_hook(|| {
        *init.made.editor.borrow_mut() = Some(ed.clone());
        init.made.chrome.set(Some(chrome));
    });
    let appearance = app::settings().appearance;
    rsx! {
        UnitRoot { keys: Keys::Host, appearance: unit_appearance(appearance), theme: unit_theme(),
            KeepUnit { cell: init.made.unit.clone() }
            {ed.render()}
            style { {themed(crate::toolbar::CSS, appearance)} }
            {crate::toolbar::render(&ed, &chrome.read())}
        }
    }
}

/// unit's colours, which are Leaf's own, with the page's background left to
/// the page: the editor's, or the Settings window's.
pub fn unit_theme() -> Theme {
    let mut theme = Theme::default();
    theme.light.bg = "transparent";
    theme.dark.bg = "transparent";
    theme
}

/// The reader's appearance, as unit's root is told it. A forced one cannot
/// go through `prefers-color-scheme` (`leaf_dioxus::themed` says why).
pub fn unit_appearance(a: leaf_dioxus::Appearance) -> unit::Appearance {
    match a {
        leaf_dioxus::Appearance::System => unit::Appearance::System,
        leaf_dioxus::Appearance::Light => unit::Appearance::Light,
        leaf_dioxus::Appearance::Dark => unit::Appearance::Dark,
    }
}

#[derive(Props, Clone)]
pub struct KeepUnitProps {
    cell: Rc<Cell<Option<Unit>>>,
}

impl PartialEq for KeepUnitProps {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.cell, &other.cell)
    }
}

/// Hands the page's [`Unit`] to the app, which gives it the window's keys
/// first, and gives it the system's clipboard for a dialog's field.
#[allow(non_snake_case)]
pub fn KeepUnit(props: KeepUnitProps) -> Element {
    let unit = use_unit();
    use_hook(|| {
        unit.set_clipboard(Rc::new(crate::shell::SystemClipboard));
        props.cell.set(Some(unit));
    });
    rsx! {}
}

/// What a window shows.
pub enum PageKind {
    Document(DocInit),
    /// The Settings window, and its overlay once made.
    Settings(Made),
}

/// A window the app has open, and what it shows.
pub struct Page {
    pub window: Arc<dyn Window>,
    runtime: Rc<Runtime>,
    pub kind: PageKind,
    /// The chrome as last read, for the system's menus.
    chrome: RefCell<Option<Chrome>>,
    /// The revision the platform was last told was an edit.
    reported: Cell<u64>,
    /// An input method's composition is in progress: what it commits is
    /// not typing the spelling checker substitutes after.
    composing: Cell<bool>,
    /// Where the pointer last was, in the page's CSS pixels.
    pointer: Cell<Option<(f64, f64)>>,
}

impl Page {
    pub fn new(window: Arc<dyn Window>, runtime: Rc<Runtime>, kind: PageKind) -> Self {
        Self {
            window,
            runtime,
            kind,
            chrome: RefCell::default(),
            reported: Cell::new(0),
            composing: Cell::new(false),
            pointer: Cell::new(None),
        }
    }

    pub fn is_settings(&self) -> bool {
        matches!(self.kind, PageKind::Settings(_))
    }

    fn init(&self) -> Option<&DocInit> {
        match &self.kind {
            PageKind::Document(i) => Some(i),
            PageKind::Settings(_) => None,
        }
    }

    fn made(&self) -> &Made {
        match &self.kind {
            PageKind::Document(i) => &i.made,
            PageKind::Settings(m) => m,
        }
    }

    pub fn shell(&self) -> Option<&Rc<Shell>> {
        self.init().map(|i| &i.shell)
    }

    /// The editor, if the page has made it.
    pub fn editor(&self) -> Option<Editor> {
        self.init()?.made.editor.borrow().clone()
    }

    /// The page's [`Unit`], once made.
    pub fn unit(&self) -> Option<Unit> {
        self.made().unit.get()
    }

    /// Where the pointer last was over the page, in CSS pixels.
    pub fn pointer(&self) -> Option<(f64, f64)> {
        self.pointer.get()
    }

    /// Run `f` inside the page's Dioxus runtime, where signals can be
    /// written and tasks spawned; the page is polled after.
    pub fn in_scope<R>(&self, f: impl FnOnce() -> R) -> R {
        let r = self.runtime.in_scope(ScopeId::ROOT, f);
        app::poll(self.window.id());
        r
    }

    /// Run `f` on the editor, inside its runtime.
    pub fn with_editor<R>(&self, f: impl FnOnce(&Editor, &Shell) -> R) -> Option<R> {
        let ed = self.editor()?;
        let shell = self.shell()?.clone();
        Some(self.in_scope(|| f(&ed, &shell)))
    }

    /// Read or change the document; `None` without one, or while the
    /// editor has it borrowed this instant.
    pub fn with_doc<R>(&self, f: impl FnOnce(&mut Doc) -> R) -> Option<R> {
        let mut d = self.init()?.doc.try_borrow_mut().ok()?;
        Some(f(&mut d))
    }

    /// Say `message` in the status line.
    pub fn say(&self, message: &str) {
        let m = message.to_string();
        self.with_editor(|ed, _| {
            ed.doc().status = Some(m);
            ed.refresh();
        });
    }

    /// The chrome as last read.
    pub fn chrome(&self) -> Option<Chrome> {
        self.chrome.borrow().clone()
    }

    /// A window event, before Blitz sees it: whether the page took it.
    /// The page's controls have the keys first — an open menu or dialog,
    /// then a control the keys were given to by a press or Tab — and the
    /// input method's text while a menu or dialog is open; a text field of
    /// the page's own types through Blitz; then a document's editor, which
    /// reads keys from winit rather than Blitz (the widget's `keys.rs` says
    /// why). The rest the editor reads and Blitz has too.
    pub fn window_event(&self, event: &WindowEvent, mods: ModifiersState) -> bool {
        match event {
            WindowEvent::KeyboardInput { event: key, .. } => {
                let Some((k, code, m)) = pressed(key, mods) else {
                    return true;
                };
                if let Some(unit) = self.unit() {
                    if (unit.is_open() || unit.focused().is_some())
                        && self.in_scope(|| unit.key(&k, m))
                    {
                        return true;
                    }
                    if unit.typing() {
                        return false;
                    }
                }
                if self.init().is_none() {
                    return false;
                }
                let typed =
                    (key.state == ElementState::Pressed && !mods.meta_key() && !mods.control_key())
                        .then(|| key.text.as_ref().map(|t| t.to_string()))
                        .flatten();
                self.with_editor(|ed, shell| shell.key(ed, k, code, m));
                if let Some(t) = typed {
                    platform::spelling::after_typing(self, &t);
                }
                true
            }
            WindowEvent::Ime(ime) => {
                if let (Ime::Commit(text), Some(unit)) = (ime, self.unit().filter(Unit::is_open))
                    && self.in_scope(|| unit.text(text))
                {
                    return true;
                }
                if self.unit().is_some_and(|u| u.typing()) {
                    return false;
                }
                if self.init().is_none() {
                    return true;
                }
                let commit = match ime {
                    Ime::Preedit(text, _) => {
                        self.composing.set(!text.is_empty());
                        None
                    }
                    Ime::Commit(text) => (!self.composing.replace(false)).then(|| text.clone()),
                    _ => None,
                };
                self.with_editor(|ed, _| ed.window_event(event));
                if let Some(t) = commit {
                    platform::spelling::after_typing(self, &t);
                }
                true
            }
            // A force click: the trackpad's Look Up, where the system gives
            // Look Up to one.
            WindowEvent::TouchpadPressure { stage, .. } => {
                if *stage == 2 && self.init().is_some() {
                    platform::lookup::force_click(self);
                }
                false
            }
            WindowEvent::PointerMoved { position, .. } => {
                let s = self.window.scale_factor();
                self.pointer.set(Some((position.x / s, position.y / s)));
                self.with_editor(|ed, _| ed.window_event(event));
                false
            }
            _ => {
                self.with_editor(|ed, _| ed.window_event(event));
                false
            }
        }
    }

    /// After Blitz has had an event: a new size moves the editor's box, and
    /// the overlay's bounds.
    pub fn after_event(&self, event: &WindowEvent) {
        if let WindowEvent::SurfaceResized(_) = event {
            self.place();
        }
    }

    /// The page's size in CSS pixels.
    pub fn size(&self) -> (f64, f64) {
        let size = self.window.surface_size();
        let s = self.window.scale_factor();
        (size.width as f64 / s, size.height as f64 / s)
    }

    /// Put the editor's box below the toolbar, the window's width, and tell
    /// the overlay the page's size.
    pub fn place(&self) {
        let (w, h) = self.size();
        if let Some(unit) = self.unit() {
            self.in_scope(|| unit.set_viewport(w, h));
        }
        let top = chrome_height();
        self.with_editor(|ed, _| {
            ed.set_bounds(Some(Bounds {
                left: 0.0,
                top,
                width: w,
                height: (h - top).max(0.0),
            }))
        });
    }

    /// Bring the chrome up to date with the document, if a repaint since
    /// the last time may have changed it; and tell the platform of an edit.
    pub fn sync(&self) {
        let Some(init) = self.init() else { return };
        if !init.shell.stale.replace(false) {
            return;
        }
        let Some(chrome) = self.with_doc(Chrome::of) else {
            init.shell.stale.set(true);
            return;
        };
        if chrome.dirty && chrome.revision != self.reported.get() {
            self.reported.set(chrome.revision);
            init.shell.document.edited();
        }
        platform::spelling::changed(self, &chrome);
        if self.chrome.borrow().as_ref() == Some(&chrome) {
            return;
        }
        *self.chrome.borrow_mut() = Some(chrome.clone());
        if let Some(mut sig) = init.made.chrome.get() {
            self.in_scope(|| sig.set(chrome));
        }
    }

    /// The source for the platform to write, and the document clean as of
    /// it; `None` if the editor has it borrowed this instant.
    pub fn source_to_save(&self) -> Option<String> {
        let source = self.with_doc(|d| {
            d.dirty = false;
            d.source.clone()
        })?;
        if let Some(s) = self.shell() {
            s.stale.set(true);
        }
        app::post(AppEvent::Refresh(self.window.id()));
        Some(source)
    }

    /// Redraw: the status line says what changed outside the editor.
    pub fn refresh(&self) {
        self.with_editor(|ed, _| ed.refresh());
    }

    /// The document read its file again.
    pub fn reload(&self, source: String) {
        let Some(init) = self.init() else { return };
        let mut doc = init.shell.document.take_doc_from(source);
        doc.dirty = false;
        self.with_editor(|ed, _| ed.install(doc));
    }

    /// The document's file moved or was named.
    pub fn moved(&self) {
        let Some(init) = self.init() else { return };
        let path = init.shell.document.path();
        self.with_editor(|ed, _| {
            ed.doc().path = path;
            ed.refresh();
        });
    }

    /// The display choices changed.
    pub fn settings_changed(&self) {
        use dioxus_native::winit::window::Theme;
        let theme = match app::settings().appearance {
            leaf_dioxus::Appearance::System => None,
            leaf_dioxus::Appearance::Light => Some(Theme::Light),
            leaf_dioxus::Appearance::Dark => Some(Theme::Dark),
        };
        self.window.set_theme(theme);
        self.with_editor(|ed, shell| ed.set_look(shell.look()));
        app::poll(self.window.id());
    }

    /// The window's close button, or File ▸ Close. The platform's document
    /// has been asked already where it has a say (`platform::Document`
    /// says when), so the window goes.
    pub fn close_requested(&self) {
        leaf_dioxus::trace(|| format!("close requested {:?}", self.window.id()));
        app::post(AppEvent::Close(self.window.id()));
    }

    /// The window has gone: so does its document.
    pub fn closed(&self) {
        if let Some(init) = self.init() {
            platform::spelling::closed(self.window.id());
            init.shell.document.close();
        }
    }

    /// A command from a menu or the toolbar, for this window.
    pub fn command(&self, c: Command) {
        match c {
            Command::Link => crate::context::prompt_link(self),
            Command::ExportPdf => platform::print::export(self),
            Command::CheckDocument => platform::spelling::check_document(self),
            Command::Replace(loc, len, word, with) => {
                platform::spelling::replace(self, loc, len, &word, &with)
            }
            Command::IgnoreSpelling(w) => platform::spelling::ignore(self, &w),
            Command::LearnSpelling(w) => platform::spelling::learn(self, &w),
            Command::LookUp(loc, len) => platform::lookup::show(self, loc, len),
            c if c.is_app() => app::run_app(c),
            c => {
                self.with_editor(|ed, shell| match c {
                    Command::Chord(k, code, m) => shell.key(ed, k, code, m),
                    Command::Table(t) => ed.table(t),
                    Command::Highlight(colour) => edit(ed, |d| d.highlight(colour)),
                    Command::OpenLink => {
                        ed.open_link();
                    }
                    Command::CopyLink => ed.copy_link(),
                    Command::FollowFootnote => {
                        ed.follow_footnote();
                    }
                    Command::InsertFootnote => edit(ed, Doc::insert_footnote),
                    Command::InsertRule => edit(ed, Doc::insert_thematic_break),
                    Command::InsertPageBreak => edit(ed, Doc::insert_page_break),
                    Command::Verse => edit(ed, Doc::toggle_verse),
                    Command::PreserveBreaks => edit(ed, |d| {
                        let flow = match d.line_flow() {
                            LineFlow::Preserve => LineFlow::Fold,
                            _ => LineFlow::Preserve,
                        };
                        d.set_line_flow(flow);
                    }),
                    Command::Zoom(z) => shell.zoom(ed, z),
                    _ => {}
                });
            }
        }
    }
}

/// An edit no key stands for, then a repaint.
fn edit(ed: &Editor, f: impl FnOnce(&mut Doc)) {
    f(&mut ed.doc());
    ed.refresh();
}
