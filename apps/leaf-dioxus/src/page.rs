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
//! With the PDF preview shown, the editor has the window's left half and
//! the pages its right; in the page view the pages are over the editor,
//! which still has the keys and the document (`preview.rs`, `pages.rs`).
//!
//! What the menus and the toolbar show — the marks in force, the heading,
//! whether there is anything to undo — is the [`Chrome`], read off the
//! document after every turn that repainted it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dioxus::prelude::*;
use dioxus_core::{Runtime, ScopeId};
use dioxus_native::winit::event::{ElementState, Ime, MouseScrollDelta, TouchPhase, WindowEvent};
use dioxus_native::winit::keyboard::ModifiersState;
use dioxus_native::winit::window::Window;
use leaf_core::{Doc, InlineMarks, LineFlow, MarkColor, View};
use leaf_dioxus::{Bounds, Editor, Host, pressed, themed, use_shared_editor};
use unit::{Keys, Theme, Unit, UnitRoot, use_unit};

use crate::app::{self, AppEvent};
use crate::commands::Command;
use crate::pages::PagesView;
use crate::platform;
use crate::preview::{self, Mode, Preview};
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
    /// The page view or the reflowed view is shown: the window's, not the
    /// document's. The source view, beside the pages, is `source_view`.
    pub pages: bool,
    pub reflow: bool,
    /// The app's menus as of this chrome (`menus::generation`): a toolbar
    /// menu is built again when a choice it ticks, or an item it lists,
    /// changes.
    pub menus: u64,
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
            pages: false,
            reflow: false,
            menus: crate::menus::generation(),
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
    pub view: Rc<Cell<Option<Signal<PagesView>>>>,
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
    let view = use_signal(PagesView::default);
    let ed = use_shared_editor(
        || init.doc.clone(),
        || init.shell.look(),
        || init.shell.clone() as Rc<dyn Host>,
    );
    use_hook(|| {
        *init.made.editor.borrow_mut() = Some(ed.clone());
        init.made.chrome.set(Some(chrome));
        init.made.view.set(Some(view));
    });
    let appearance = app::settings().appearance;
    rsx! {
        UnitRoot { keys: Keys::Host, appearance: unit_appearance(appearance), theme: unit_theme(),
            KeepUnit { cell: init.made.unit.clone() }
            {ed.render()}
            style { {themed(crate::toolbar::CSS, appearance)} }
            {crate::toolbar::render(&ed, &chrome.read())}
            style { {themed(crate::pages::CSS, appearance)} }
            crate::pages::Pages { view }
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
    /// A touchpad's scroll under way, for the throw it leaves, and the view
    /// its fingers hold.
    touchpad: RefCell<Touchpad>,
    held: Cell<Option<Held>>,
    /// When the page was made: what the wheel's times count from.
    since: std::time::Instant,
    /// The pages beside a document's editor, or in its place.
    pub preview: Preview,
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
            touchpad: RefCell::default(),
            held: Cell::new(None),
            since: std::time::Instant::now(),
            preview: Preview::default(),
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

    /// Change what the pages' view draws, inside the page's runtime; not
    /// without a document's page.
    pub fn with_view(&self, f: impl FnOnce(Signal<PagesView>)) {
        if let Some(view) = self.init().and_then(|i| i.made.view.get()) {
            self.in_scope(|| f(view));
        }
    }

    /// Read what the pages' view draws; the default without a document's
    /// page.
    pub fn with_view_peek<R: Default>(&self, f: impl FnOnce(&PagesView) -> R) -> R {
        match self.init().and_then(|i| i.made.view.get()) {
            Some(view) => f(&view.peek()),
            None => R::default(),
        }
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
                // Where there is no menu bar, the keys of its commands.
                if platform::shortcut(self, &code, m) {
                    return true;
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
            // A trackpad's pinch, over the pages: they zoom about the
            // pointer.
            WindowEvent::PinchGesture { delta, .. } => preview::pinch(self, *delta, self.pointer()),
            WindowEvent::MouseWheel { delta, phase, .. } => self.touchpad(delta, *phase),
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

    /// A touchpad's scroll, which the page takes from Blitz where the
    /// system says when the fingers come down and lift (Wayland, not X11):
    /// the view under the pointer, the editor or the pages, follows the
    /// fingers, stretching past either end as a phone's does, and as they
    /// lift carries on, or springs back to the end it was pulled past. Off
    /// the Mac, whose system does all of this itself. A wheel's notch stops
    /// a throw under way, and Blitz scrolls by it. Whether the page took the
    /// event.
    fn touchpad(&self, delta: &MouseScrollDelta, phase: TouchPhase) -> bool {
        if cfg!(target_os = "macos") {
            return false;
        }
        let now = self.since.elapsed().as_secs_f64() * 1000.0;
        let scale = self.window.scale_factor();
        // A wheel's notch counts in lines; a touchpad's travel in pixels.
        let (dx, dy) = match delta {
            MouseScrollDelta::PixelDelta(d) => (d.x / scale, Some(d.y / scale)),
            _ => (0.0, None),
        };
        let wheeled = self.touchpad.borrow_mut().wheel(now, dy, phase);
        match wheeled {
            Wheeled::Notch => {
                self.stop_throws();
                false
            }
            Wheeled::Begin(travelled) => {
                self.stop_throws();
                let pages = self.pointer().is_some_and(|at| preview::under(self, at));
                let from = if pages {
                    self.with_view_peek(|v| v.scroll.y)
                } else {
                    self.editor().map_or(0.0, |ed| ed.top())
                };
                self.held.set(Some(Held { pages, from }));
                self.pull(travelled, dx);
                self.settle_after_pause(now);
                true
            }
            Wheeled::Pull(travelled) => {
                if self.held.get().is_none() {
                    return false;
                }
                self.pull(travelled, dx);
                self.settle_after_pause(now);
                true
            }
            Wheeled::Throw(vel) => {
                let Some(held) = self.held.take() else {
                    return false;
                };
                if held.pages {
                    preview::fling(self, vel);
                } else {
                    self.with_editor(|ed, _| ed.start_glide(vel));
                }
                true
            }
            Wheeled::Ignored => false,
        }
    }

    /// A scroll that has gone quiet since `at` (milliseconds) by the time a
    /// pause is over has ended, whether or not the system said so: the view
    /// it pulled past an end springs back, with no throw.
    fn settle_after_pause(&self, at: f64) {
        let id = self.window.id();
        self.in_scope(|| {
            spawn(async move {
                let wait = std::time::Duration::from_millis(PAUSE as u64 + 10);
                tokio::time::sleep(wait).await;
                let Some(page) = app::page(id) else { return };
                if page.touchpad.borrow().last != at || page.held.get().is_none() {
                    return;
                }
                page.touchpad.borrow_mut().down = false;
                let held = page.held.take().expect("checked above");
                if held.pages {
                    preview::fling(&page, 0.0);
                } else {
                    page.with_editor(|ed, _| ed.start_glide(0.0));
                }
            });
        });
    }

    /// The view the fingers hold, moved `travelled` CSS pixels (positive
    /// toward the top) from where they came down: past either end, the
    /// rubber band's share of it. The pages move across by `dx` too, within
    /// their ends.
    fn pull(&self, travelled: f64, dx: f64) {
        let Some(held) = self.held.get() else { return };
        let raw = held.from - travelled;
        if held.pages {
            self.with_view(|mut v| {
                let mut v = v.write();
                let ((x0, x1), (lo, hi)) = v.scroll.range();
                let d = v.scroll.view.1;
                v.scroll.y = unit::scroll::rubber(raw, lo, hi, d);
                v.scroll.x = (v.scroll.x - dx).clamp(x0, x1);
            });
        } else {
            self.with_editor(|ed, _| {
                let max = ed.max_top().max(0.0);
                ed.scroll_to(unit::scroll::rubber(raw, 0.0, max, ed.bounds().height));
            });
        }
    }

    /// Stop a throw under way, the editor's or the pages'.
    fn stop_throws(&self) {
        preview::stop_fling(self);
        self.with_editor(|ed, _| ed.stop_glide());
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

    /// Put the editor's box below the toolbar, the window's width less the
    /// PDF preview's, and tell the overlay the page's size.
    pub fn place(&self) {
        let (w, h) = self.size();
        if let Some(unit) = self.unit() {
            self.in_scope(|| unit.set_viewport(w, h));
        }
        let top = chrome_height();
        let pane = preview::place(self, w);
        self.with_editor(|ed, _| {
            ed.set_bounds(Some(Bounds {
                left: 0.0,
                top,
                width: w - pane,
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
        let t0 = std::time::Instant::now();
        let Some(mut chrome) = self.with_doc(Chrome::of) else {
            init.shell.stale.set(true);
            return;
        };
        let t_chrome = t0.elapsed();
        // ⌘E, or the editor's own command, turns the source view on or off:
        // the window shows it beside the pages, or goes back.
        preview::follow(self, chrome.source_view);
        chrome.pages = self.preview.mode() == Mode::Pages;
        chrome.reflow = self.preview.mode() == Mode::Reflow;
        preview::update(self);
        if chrome.dirty && chrome.revision != self.reported.get() {
            self.reported.set(chrome.revision);
            init.shell.document.edited();
        } else if !chrome.dirty {
            init.shell.document.clean();
        }
        let t_update = t0.elapsed();
        platform::spelling::changed(self, &chrome);
        leaf_dioxus::trace(|| {
            format!(
                "sync: chrome {t_chrome:.1?}, pages {t_update:.1?}, all {:.1?}",
                t0.elapsed()
            )
        });
        if self.chrome.borrow().as_ref() == Some(&chrome) {
            return;
        }
        *self.chrome.borrow_mut() = Some(chrome.clone());
        if let Some(mut sig) = init.made.chrome.get() {
            self.in_scope(|| sig.set(chrome));
        }
    }

    /// The source for the platform to write, and the document clean as of
    /// it; `None` if the editor has it borrowed this instant. The Mac's
    /// document system asks for it; Linux's saves through leaf-core instead.
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

    /// The window came forward: the document looks at its file, where the
    /// platform leaves that to the app.
    pub fn focused(&self) {
        if let Some(init) = self.init() {
            init.shell.document.focused(self);
        }
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
        // The toolbar's menus tick the choices.
        if let Some(init) = self.init() {
            init.shell.stale.set(true);
        }
        preview::update(self);
        app::poll(self.window.id());
    }

    /// The window's close button, or File ▸ Close: a document's window
    /// goes once its platform document agrees (on the Mac it has been asked
    /// already; elsewhere it asks if changes would be lost), Settings'
    /// at once.
    pub fn close_requested(&self) {
        leaf_dioxus::trace(|| format!("close requested {:?}", self.window.id()));
        match self.init() {
            Some(init) => init.shell.document.ask_to_close(self),
            None => app::post(AppEvent::Close(self.window.id())),
        }
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
            Command::InstallTemplate => crate::templates::prompt_install(self),
            Command::ExportPdf => platform::print::export(self),
            Command::PageView => preview::show(self, Mode::Pages),
            Command::Reflow => preview::show(self, Mode::Reflow),
            Command::Save => {
                if let Some(s) = self.shell() {
                    s.document.save();
                }
            }
            Command::SaveAs => {
                if let Some(s) = self.shell() {
                    s.document.save_as();
                }
            }
            Command::Close => self.close_requested(),
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

/// A touchpad's scroll, followed for the throw its fingers leave as they
/// lift: their travel, and its recent times.
#[derive(Default)]
struct Touchpad {
    tracker: unit::scroll::Tracker,
    travelled: f64,
    /// Fingers came down, and have not lifted.
    down: bool,
    /// When the last event came, in milliseconds.
    last: f64,
}

/// The longest pause between a scroll's events, in milliseconds: one that
/// comes after a longer one begins a scroll of its own. Winit does not
/// promise a lift for every device (a mouse's, or a virtual pointer's,
/// smooth scroll never says its fingers lifted), and a scroll that never
/// ended would hold the view it began on for good.
const PAUSE: f64 = 150.0;

/// The view a touchpad's fingers hold, and its offset when they came down.
#[derive(Clone, Copy, Debug)]
struct Held {
    pages: bool,
    from: f64,
}

/// What a wheel event asks of the views.
#[derive(Debug, PartialEq)]
enum Wheeled {
    /// A wheel's notch: stop a throw under way, and let Blitz scroll.
    Notch,
    /// Fingers came down to scroll, and have moved this far (positive
    /// toward the top) since.
    Begin(f64),
    /// They have moved this far since they came down.
    Pull(f64),
    /// They lifted: throw the view at this many CSS pixels a millisecond,
    /// down the offset, which may be none, to spring back to an end.
    Throw(f64),
    /// A scroll whose fingers were never seen to come down (X11's): Blitz's.
    Ignored,
}

impl Touchpad {
    /// A wheel event at `now` (milliseconds): a touchpad's `dy` in CSS
    /// pixels (positive toward the top, as Blitz has it), or a notch's
    /// (`None`), in its scroll's `phase`.
    fn wheel(&mut self, now: f64, dy: Option<f64>, phase: TouchPhase) -> Wheeled {
        let Some(dy) = dy else { return Wheeled::Notch };
        let paused = now - self.last > PAUSE;
        self.last = now;
        let phase = match phase {
            TouchPhase::Moved if paused => TouchPhase::Started,
            phase => phase,
        };
        match phase {
            TouchPhase::Started => {
                self.tracker.reset();
                self.travelled = dy;
                self.tracker.push(now, dy);
                self.down = true;
                Wheeled::Begin(self.travelled)
            }
            TouchPhase::Moved if self.down => {
                self.travelled += dy;
                self.tracker.push(now, self.travelled);
                Wheeled::Pull(self.travelled)
            }
            TouchPhase::Ended | TouchPhase::Cancelled if self.down => {
                self.down = false;
                // A positive delta moves toward the top: the offset falls.
                let vel = -self.tracker.velocity(now);
                Wheeled::Throw(if vel.abs() < 0.05 { 0.0 } else { vel })
            }
            _ => Wheeled::Ignored,
        }
    }
}

/// An edit no key stands for, then a repaint.
fn edit(ed: &Editor, f: impl FnOnce(&mut Doc)) {
    f(&mut ed.doc());
    ed.refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fingers moving up the pad at a steady 1 px/ms (the page going down)
    /// pull the view as far as they go and throw it on as they lift;
    /// fingers that rested before they lifted throw nothing; a notch is the
    /// wheel's, and a scroll whose fingers were never seen to come down is
    /// Blitz's.
    #[test]
    fn a_touchpad_pulls_the_view_and_throws_it() {
        let mut t = Touchpad::default();
        assert_eq!(
            t.wheel(0.0, Some(-8.0), TouchPhase::Moved),
            Wheeled::Ignored
        );
        assert_eq!(
            t.wheel(0.0, Some(-8.0), TouchPhase::Started),
            Wheeled::Begin(-8.0)
        );
        for i in 1..=10 {
            let pulled = t.wheel(i as f64 * 8.0, Some(-8.0), TouchPhase::Moved);
            assert_eq!(pulled, Wheeled::Pull(-8.0 * (i + 1) as f64));
        }
        match t.wheel(84.0, Some(0.0), TouchPhase::Ended) {
            Wheeled::Throw(v) => assert!((v - 1.0).abs() < 0.05, "{v}"),
            w => panic!("no throw: {w:?}"),
        }
        assert_eq!(
            t.wheel(90.0, Some(-8.0), TouchPhase::Moved),
            Wheeled::Ignored
        );

        t.wheel(200.0, Some(-8.0), TouchPhase::Started);
        t.wheel(208.0, Some(-8.0), TouchPhase::Moved);
        assert_eq!(
            t.wheel(400.0, Some(0.0), TouchPhase::Ended),
            Wheeled::Throw(0.0)
        );

        assert_eq!(t.wheel(500.0, None, TouchPhase::Moved), Wheeled::Notch);
    }

    /// A smooth scroll whose fingers are never seen to lift (a mouse's, a
    /// virtual pointer's) begins again after a pause, so it holds the view
    /// under the pointer now, not the one it first began on.
    #[test]
    fn a_scroll_that_never_lifts_begins_again_after_a_pause() {
        let mut t = Touchpad::default();
        assert_eq!(
            t.wheel(1000.0, Some(-8.0), TouchPhase::Started),
            Wheeled::Begin(-8.0)
        );
        assert_eq!(
            t.wheel(1008.0, Some(-8.0), TouchPhase::Moved),
            Wheeled::Pull(-16.0)
        );
        assert_eq!(
            t.wheel(2000.0, Some(-8.0), TouchPhase::Moved),
            Wheeled::Begin(-8.0)
        );
    }
}
