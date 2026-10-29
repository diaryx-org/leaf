//! The event loop's handler: what dioxus-native's `launch` does for one
//! window, done here for every window the app has open.
//!
//! `launch` builds one `VirtualDom`, one Blitz document and one winit window
//! before the loop starts, and has no way to make another after. A document
//! app makes a window whenever the system opens a document — at launch,
//! from the Open panel, from Finder — so this owns blitz-shell's
//! [`BlitzApplication`] itself and assembles each window's page as `launch`
//! would ([`App::make_window`]), on the handler's turn, where an
//! `ActiveEventLoop` is to hand.
//!
//! Everything the platform tells the app — a document wanting its window,
//! a menu item chosen, a revert, the spelling checker's answer — arrives
//! outside that turn. It is [`post`]ed as an [`AppEvent`], or queued with
//! [`on_turn`], and the handler takes it on its next turn. So the editor is
//! only ever touched from the handler, never while Blitz holds its document
//! borrowed.
//!
//! Each window's events go to its page before Blitz sees them, as
//! `use_window_event` handlers do under `launch`: the keys to the overlay
//! and the editor, the rest to the editor and then Blitz.

use std::any::Any;
use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use blitz_dom::DocumentConfig;
use blitz_shell::{BlitzApplication, BlitzShellEvent, BlitzShellProxy, View, WindowConfig};
use dioxus::prelude::*;
use dioxus_core::{ScopeId, VirtualDom};
use dioxus_native::DioxusDocument;
use dioxus_native::LogicalSize;
use dioxus_native::winit::application::ApplicationHandler;
use dioxus_native::winit::data_transfer::TypeHint;
use dioxus_native::winit::event::{StartCause, WindowEvent};
use dioxus_native::winit::event_loop::{
    ActiveEventLoop, AsyncRequestSerial, ControlFlow, DndAction, EventLoop, EventLoopProxy,
    EventLoopProxyProvider,
};
use dioxus_native::winit::keyboard::ModifiersState;
use dioxus_native::winit::window::{Window, WindowAttributes, WindowButtons, WindowId};
use leaf_dioxus::Context;

use crate::commands::{AppState, Command};
use crate::guard::guard;
use crate::page::{DocInit, Made, Page, PageKind};
use crate::platform::{self, DocumentRef};
use crate::renderer::Renderer;
use crate::settings::Settings;
use crate::shell::Shell;

/// What the platform asked for, outside the handler's turn.
pub enum AppEvent {
    /// A document was opened or made and wants its window.
    Show(DocumentRef),
    /// A document read its file again — Revert, or a change made outside
    /// the app — and its window shows the new source.
    Reload(WindowId, String),
    /// A document's file moved or was named: its pictures' paths follow.
    Moved(WindowId),
    /// Redraw a window: something outside its editor changed what it shows.
    Refresh(WindowId),
    /// A command, for the window it acts on.
    Command(WindowId, Command),
    /// The editor asked for its context menu.
    ContextMenu(WindowId, Context),
    /// Settings, made or brought forward.
    ShowSettings,
    /// A display choice changed: every window draws with it.
    SettingsChanged,
    /// A document may close, and its window goes.
    Close(WindowId),
    /// Put an element in a window's `<head>` (`head.rs`).
    Head(WindowId, String, Vec<(String, String)>, Option<String>),
    /// Run this on the handler's turn.
    Run(Box<dyn FnOnce()>),
}

thread_local! {
    static QUEUE: RefCell<VecDeque<AppEvent>> = RefCell::default();
    static PROXY: OnceCell<BlitzShellProxy> = const { OnceCell::new() };
    static PAGES: RefCell<HashMap<WindowId, Rc<Page>>> = RefCell::default();
    /// The window that last had focus.
    static FOCUSED: std::cell::Cell<Option<WindowId>> = const { std::cell::Cell::new(None) };
    /// The display choices every window shares.
    static SETTINGS: RefCell<Settings> = RefCell::new(Settings::load());
}

/// Ask the handler for `event` on its next turn.
pub fn post(event: AppEvent) {
    QUEUE.with_borrow_mut(|q| q.push_back(event));
    proxy().send_event(BlitzShellEvent::embedder_event(()));
}

/// Run `f` on the handler's next turn.
pub fn on_turn(f: impl FnOnce() + 'static) {
    post(AppEvent::Run(Box::new(f)));
}

/// A closure parked on the handler's thread until another thread answers
/// it ([`park`], [`unpark`]): a system dialog, run off the handler so the
/// windows keep drawing while it is up.
type Parked = Box<dyn FnOnce(Box<dyn Any + Send>)>;

thread_local! {
    static PARKED: RefCell<HashMap<u64, Parked>> = RefCell::default();
    static NEXT_PARKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Where another thread sends a parked closure its answer.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct Token {
    id: u64,
    proxy: BlitzShellProxy,
}

/// An answer for a parked closure, as it crosses to the handler's thread.
struct Answer {
    id: u64,
    value: std::sync::Mutex<Option<Box<dyn Any + Send>>>,
}

/// Park `then` until another thread [`unpark`]s the token with its answer;
/// it runs on the handler's turn.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn park<T: Send + 'static>(then: impl FnOnce(T) + 'static) -> Token {
    let id = NEXT_PARKED.replace(NEXT_PARKED.get() + 1);
    let parked: Parked = Box::new(move |value| {
        if let Ok(value) = value.downcast::<T>() {
            then(*value);
        }
    });
    PARKED.with_borrow_mut(|p| p.insert(id, parked));
    Token { id, proxy: proxy() }
}

/// Answer a parked closure, from any thread.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn unpark<T: Send + 'static>(token: Token, answer: T) {
    token
        .proxy
        .send_event(BlitzShellEvent::embedder_event(Answer {
            id: token.id,
            value: std::sync::Mutex::new(Some(Box::new(answer))),
        }));
}

/// Run the closure an answer is for.
fn answered(event: &(dyn Any + Send + Sync)) {
    let Some(answer) = event.downcast_ref::<Answer>() else {
        return;
    };
    let value = answer.value.lock().ok().and_then(|mut v| v.take());
    let parked = PARKED.with_borrow_mut(|p| p.remove(&answer.id));
    if let (Some(value), Some(parked)) = (value, parked) {
        parked(value);
    }
}

/// Ask Blitz to poll `id`'s page: what an editor call changed is drawn.
pub fn poll(id: WindowId) {
    proxy().send_event(BlitzShellEvent::Poll { window_id: id });
}

/// A window's network provider: blitz-net's, with the pages' `data:` URLs
/// loaded in order (`leaf_pages::net`); and blitz-net's own under it, which
/// the window's root context holds.
pub fn net(
    waker: Arc<dyn blitz_traits::net::NetWaker>,
) -> (Arc<leaf_pages::net::Ordered>, Arc<blitz_net::Provider>) {
    let inner = Arc::new(blitz_net::Provider::new(Some(Arc::clone(&waker))));
    let net = leaf_pages::net::Ordered::new(inner.clone(), waker);
    (net, inner)
}

/// The loop's proxy — or, with no loop (a test laying out a PDF), one that
/// wakes nothing, with a tokio runtime entered for Blitz's network provider.
pub fn proxy() -> BlitzShellProxy {
    PROXY.with(|p| {
        p.get_or_init(|| {
            #[derive(Debug)]
            struct Nowhere;
            impl EventLoopProxyProvider for Nowhere {
                fn wake_up(&self) {}
            }
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime");
            std::mem::forget(rt.enter());
            std::mem::forget(rt);
            BlitzShellProxy::new(EventLoopProxy::new(Arc::new(Nowhere))).0
        })
        .clone()
    })
}

/// The page in window `id`.
pub fn page(id: WindowId) -> Option<Rc<Page>> {
    PAGES.with_borrow(|p| p.get(&id).cloned())
}

/// Every page open.
pub fn pages() -> Vec<Rc<Page>> {
    PAGES.with_borrow(|p| p.values().cloned().collect())
}

/// Whether the app has a page by this id still: a platform callback's
/// window may have closed since it was queued.
pub fn is_open(id: WindowId) -> bool {
    PAGES.with_borrow(|p| p.contains_key(&id))
}

/// The window that last had focus, if any has.
pub fn last_focused() -> Option<WindowId> {
    FOCUSED.get()
}

/// The display choices.
pub fn settings() -> Settings {
    SETTINGS.with_borrow(Settings::clone)
}

/// Change the display choices, store them, and redraw every window.
pub fn set_settings(s: Settings) {
    if SETTINGS.with_borrow(|now| *now == s) {
        return;
    }
    s.store();
    SETTINGS.with_borrow_mut(|now| *now = s);
    crate::menus::changed();
    post(AppEvent::SettingsChanged);
}

/// What the app holds that menus tick.
pub fn app_state() -> AppState {
    let s = settings();
    AppState {
        switches: platform::spelling::switches(),
        column: s.column,
        size: s.size,
        template: s.template,
    }
}

/// An app command, done now.
pub fn run_app(c: Command) {
    match c {
        Command::New(format) => platform::new_document(format),
        Command::Open => platform::open_panel(),
        Command::Quit => platform::quit(),
        Command::OpenSample => platform::open_sample(),
        Command::Settings => post(AppEvent::ShowSettings),
        Command::Column(column) => set_settings(Settings {
            column,
            ..settings()
        }),
        Command::Size(size) => set_settings(Settings { size, ..settings() }),
        Command::Template(template) => set_settings(Settings {
            template,
            ..settings()
        }),
        Command::Switch(s) => platform::spelling::toggle(s),
        Command::ShowSubstitutions => platform::spelling::show_substitutions(),
        _ => {}
    }
}

/// Start the app: the platform's pieces first, then the loop, which runs
/// until the app quits.
pub fn run() {
    crate::guard::install_hook();
    let mut builder = EventLoop::builder();
    platform::event_loop(&mut builder);
    let event_loop = builder.build().expect("an event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let (proxy, queue) = BlitzShellProxy::new(event_loop.create_proxy());
    PROXY.with(|p| p.set(proxy.clone()).ok());

    // Blitz's network provider reads pictures on tokio, and the editor's
    // word count waits on its timer; entered for the life of the app.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a tokio runtime");
    let _guard = rt.enter();

    #[cfg(debug_assertions)]
    crate::drive::start({
        let p = proxy.clone();
        move || p.send_event(BlitzShellEvent::embedder_event(()))
    });
    platform::start();
    let app = App {
        blitz: BlitzApplication::new(proxy, queue),
        ready: false,
        mods: HashMap::new(),
        frames: HashMap::new(),
        drops: HashMap::new(),
    };
    event_loop.run_app(app).expect("the event loop");
}

struct App {
    blitz: BlitzApplication<Renderer>,
    /// Whether the loop can make windows yet (`can_create_surfaces`).
    ready: bool,
    /// The modifiers held, per window, for the keys.
    mods: HashMap<WindowId, ModifiersState>,
    /// What each window has painted, for [`redraw`].
    frames: HashMap<WindowId, Frame>,
    /// Files dropped on a window, asked for and on their way.
    drops: HashMap<AsyncRequestSerial, WindowId>,
}

/// What a window has painted, for [`redraw`].
#[derive(Default)]
struct Frame {
    /// The viewport it last painted at — its drawable size, in physical
    /// pixels, and its scale — if it has painted at all.
    viewport: Option<((u32, u32), f32)>,
    /// Whether its page has changed since.
    changed: bool,
}

/// A new document window's size, before it has one of its own: US Letter's
/// 8.5 × 11, at a width that holds the Medium column with its margins.
const DOCUMENT_SIZE: (f64, f64) = (700.0, 906.0);
const DOCUMENT_MIN: (f64, f64) = (480.0, 320.0);

impl App {
    /// Take what the platform asked for since the last turn.
    fn drain(&mut self, event_loop: &dyn ActiveEventLoop) {
        if !self.ready {
            return;
        }
        while let Some(event) = QUEUE.with_borrow_mut(VecDeque::pop_front) {
            guard(|| self.handle(event_loop, event));
        }
    }

    fn handle(&mut self, event_loop: &dyn ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Show(document) => self.open(event_loop, document),
            AppEvent::Reload(id, source) => {
                if let Some(p) = page(id) {
                    p.reload(source);
                }
            }
            AppEvent::Moved(id) => {
                if let Some(p) = page(id) {
                    p.moved();
                }
            }
            AppEvent::Refresh(id) => {
                if let Some(p) = page(id) {
                    p.refresh();
                }
            }
            AppEvent::Command(id, c) => {
                if let Some(p) = page(id) {
                    p.command(c);
                }
            }
            AppEvent::ContextMenu(id, cx) => {
                if let Some(p) = page(id) {
                    crate::context::show(&p, &cx);
                }
            }
            AppEvent::ShowSettings => self.show_settings(event_loop),
            AppEvent::SettingsChanged => {
                for p in pages() {
                    p.settings_changed();
                }
            }
            AppEvent::Close(id) => {
                self.close(id);
                if platform::QUITS_WITH_LAST_WINDOW && !pages().iter().any(|p| !p.is_settings()) {
                    platform::clipboard::release();
                    event_loop.exit();
                }
            }
            AppEvent::Head(id, name, attributes, contents) => {
                if let Some(view) = self.blitz.windows.get_mut(&id) {
                    view.downcast_doc_mut::<DioxusDocument>()
                        .create_head_element(&name, &attributes, &contents);
                    poll(id);
                }
            }
            AppEvent::Run(f) => f(),
        }
    }

    /// Make a window: a `VirtualDom` on `root` with `contexts`, its Blitz
    /// document, and the winit window it draws in — what `launch` and
    /// dioxus-native's `can_create_surfaces` do, for any number of windows.
    /// The page is given the window and the renderer as `launch` gives
    /// them; not `launch`'s window-event handlers, since the page's events
    /// come from here.
    fn make_window(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        root: fn() -> Element,
        contexts: Vec<Box<dyn Any>>,
        attributes: WindowAttributes,
    ) -> (Arc<dyn Window>, Rc<dioxus_core::Runtime>) {
        let proxy = self.blitz.proxy.clone();
        let mut vdom = VirtualDom::new(root);
        for c in contexts {
            vdom.insert_any_root_context(c);
        }
        let (net, fetches) = net(Arc::new(proxy.clone()));
        vdom.provide_root_context(fetches);
        let document = DioxusDocument::new(
            vdom,
            DocumentConfig {
                net_provider: Some(net as _),
                ..Default::default()
            },
        );
        let runtime = document.vdom.runtime();
        let renderer = Renderer::new();
        let config =
            WindowConfig::with_attributes(Box::new(document), renderer.clone(), attributes);
        let mut view = View::init(config, event_loop, &proxy);
        let window = view.window.clone();
        {
            let doc = view.downcast_doc_mut::<DioxusDocument>();
            let w = window.clone();
            let head: Rc<dyn dioxus_document::Document> = Rc::new(crate::head::Head {
                window: window.id(),
            });
            doc.vdom.in_scope(ScopeId::ROOT, move || {
                provide_context(w);
                provide_context(renderer);
                provide_context(head);
            });
            doc.initial_build();
        }
        crate::access::install(&mut view, window.id());
        view.resume();
        view.request_redraw();
        self.blitz.windows.insert(window.id(), view);
        (window, runtime)
    }

    /// A document's window: its page, and the platform told the window is
    /// the document's.
    fn open(&mut self, event_loop: &dyn ActiveEventLoop, document: DocumentRef) {
        if document.page().is_some() {
            document.show();
            return;
        }
        let doc = document.take_doc();
        let init = DocInit::new(doc, Shell::new(document.clone()));
        let attributes = platform::document_window(
            WindowAttributes::default()
                .with_title(document.title())
                .with_surface_size(LogicalSize::new(DOCUMENT_SIZE.0, DOCUMENT_SIZE.1))
                .with_min_surface_size(LogicalSize::new(DOCUMENT_MIN.0, DOCUMENT_MIN.1))
                .with_visible(false),
        );
        let (window, runtime) = self.make_window(
            event_loop,
            crate::page::DocumentPage,
            vec![Box::new(init.clone())],
            attributes,
        );
        let id = window.id();
        FOCUSED.set(Some(id));
        init.desk.id.set(Some(id));
        let page = Rc::new(Page::new(window.clone(), runtime, PageKind::Document(init)));
        PAGES.with_borrow_mut(|p| p.insert(id, page.clone()));
        document.attach(id, &*window);
        page.place();
        page.sync();
        poll(id);
    }

    /// Settings: the one window, made or brought forward.
    fn show_settings(&mut self, event_loop: &dyn ActiveEventLoop) {
        if let Some(p) = pages().into_iter().find(|p| p.is_settings()) {
            p.window.focus_window();
            return;
        }
        let made = Made::default();
        let attributes = WindowAttributes::default()
            .with_title("Settings")
            .with_surface_size(LogicalSize::new(crate::prefs::SIZE.0, crate::prefs::SIZE.1))
            .with_resizable(false)
            .with_enabled_buttons(WindowButtons::CLOSE);
        let (window, runtime) = self.make_window(
            event_loop,
            crate::prefs::SettingsPage,
            vec![Box::new(made.clone())],
            attributes,
        );
        let id = window.id();
        let page = Rc::new(Page::new(window, runtime, PageKind::Settings(made)));
        PAGES.with_borrow_mut(|p| p.insert(id, page.clone()));
        page.place();
        poll(id);
    }

    /// The window goes: its page, its Blitz view and the winit window —
    /// dropped before the platform is told, as winit asks (its issue 4135).
    fn close(&mut self, id: WindowId) {
        let page = PAGES.with_borrow_mut(|p| p.remove(&id));
        self.mods.remove(&id);
        self.frames.remove(&id);
        if let Some(view) = self.blitz.windows.remove(&id) {
            drop(view);
        }
        if let Some(page) = page {
            page.closed();
        }
    }

    /// `LEAF_DRIVE`'s gestures, to the document window in front.
    #[cfg(debug_assertions)]
    fn drive(&mut self, event_loop: &dyn ActiveEventLoop) {
        let target = crate::drive::TARGET.lock().unwrap().clone();
        let named = target.and_then(|t| pages().into_iter().find(|p| p.window.title() == t));
        let Some(p) = named
            .or_else(|| platform::main_page().and_then(page))
            .or_else(|| pages().into_iter().find(|p| !p.is_settings()))
        else {
            return;
        };
        let id = p.window.id();
        for event in crate::drive::take(p.window.scale_factor()) {
            self.window_event(event_loop, id, event);
        }
    }

    /// After a turn: every page brings its chrome — the toolbar, the menus'
    /// ticks, the document's edited state — up to date with its editor.
    fn settle(&self) {
        for p in pages() {
            guard(|| p.sync());
        }
    }
}

/// Redraw a window — covered or not, when what it shows has changed.
///
/// Blitz paints only a window it believes visible, and believes what
/// `Occluded` last said, or, before that, the window's visibility when it
/// was made. A document window is made hidden and shown once it is placed,
/// and AppKit reports occlusion only as a change: a window shown already
/// covered — opened while another app is in front, or restored to another
/// Space — goes from unseen to unseen, and nothing is reported. So it painted
/// nothing, not its page and not at the frame its document restored, and
/// stayed blank until it was uncovered: in Mission Control, in the window
/// switcher, in a capture. (wgpu refused those frames too; the patched
/// wgpu-hal at leaf's root, `patches/wgpu-hal`, draws them.)
///
/// So a window Blitz thinks unseen still paints a frame that shows
/// something new — a new size or scale, or its page changed — and is left
/// out only of the frames that change nothing but an animation: a blinking
/// caret would otherwise keep a window no one sees drawing.
fn redraw(view: &mut View<Renderer>, frame: &mut Frame) {
    let (now, blocked) = {
        let inner = view.doc.inner();
        let viewport = inner.viewport();
        (
            (viewport.window_size, viewport.scale()),
            inner.has_pending_critical_resources(),
        )
    };
    let owed = !view.is_visible && (frame.changed || frame.viewport != Some(now));
    if owed {
        view.is_visible = true;
    }
    let start = std::time::Instant::now();
    view.handle_winit_event(WindowEvent::RedrawRequested);
    leaf_dioxus::trace(|| format!("redraw {:.1} ms", start.elapsed().as_secs_f64() * 1000.0));
    if owed {
        view.is_visible = false;
    }
    // The tree assistive tech reads, as the frame just drawn shows it: Blitz
    // rebuilds it on a poll that finds changes, which this app's own
    // resolves have often taken first. A no-op while no assistive tech is
    // listening.
    view.build_accessibility_tree();
    // Blitz paints nothing while a stylesheet or font the page waits on is
    // still loading; its arrival asks for another redraw.
    if (view.is_visible || owed) && !blocked {
        *frame = Frame {
            viewport: Some(now),
            changed: false,
        };
    }
}

impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.ready = true;
        self.blitz.can_create_surfaces(event_loop);
        platform::launched();
        self.drain(event_loop);
    }

    fn destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.destroy_surfaces(event_loop);
    }

    fn new_events(&mut self, event_loop: &dyn ActiveEventLoop, cause: StartCause) {
        self.blitz.new_events(event_loop, cause);
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let page = page(window_id);
        match &event {
            // Never Blitz's, which would drop the window and quit with the
            // last: a document is asked first, and a document app outlives
            // its windows.
            WindowEvent::CloseRequested => {
                match page {
                    Some(p) => p.close_requested(),
                    None => self.close(window_id),
                }
                return;
            }
            WindowEvent::Destroyed => return,
            // Files dragged over a document window are taken as a copy, and
            // read once they are dropped: a picture among them is put in at
            // the caret (`insert.rs`).
            WindowEvent::DragEntered { id, .. } | WindowEvent::DragPosition { id, .. }
                if page.is_some() =>
            {
                let files = event_loop
                    .data_transfer(*id)
                    .is_ok_and(|t| t.has_type(&TypeHint::UriList));
                let actions: &[DndAction] = if files { &[DndAction::Copy] } else { &[] };
                let _ = event_loop.set_valid_dnd_actions(*id, actions);
            }
            WindowEvent::DragDropped { id, .. } if page.is_some() => {
                if let Ok(serial) = event_loop.fetch_data_transfer(*id, &TypeHint::UriList) {
                    self.drops.insert(serial, window_id);
                }
            }
            WindowEvent::DataTransferReceived { serial, value, .. } => {
                if self.drops.get(serial) == Some(&window_id)
                    && let Ok(paths) = value.try_as_file_paths()
                {
                    self.drops.remove(serial);
                    if let Some(p) = &page {
                        guard(|| crate::insert::dropped(p, paths));
                    }
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods.insert(window_id, m.state());
            }
            WindowEvent::Focused(true) => {
                FOCUSED.set(Some(window_id));
                if let Some(p) = &page {
                    guard(|| p.focused());
                }
            }
            _ => {}
        }
        if let Some(p) = &page {
            let mods = self.mods.get(&window_id).copied().unwrap_or_default();
            if guard(|| p.window_event(&event, mods)).unwrap_or(true) {
                poll(window_id);
                self.settle();
                self.proxy_wake_up(event_loop);
                return;
            }
        }
        if let Some(view) = self.blitz.windows.get_mut(&window_id) {
            let frame = self.frames.entry(window_id).or_default();
            guard(|| {
                if matches!(event, WindowEvent::RedrawRequested) {
                    redraw(view, frame);
                    return;
                }
                // A change a poll made to the page is laid out at the next
                // redraw, and a pointer event before it would hit-test the
                // tree as it was, through nodes the change removed — a
                // panic in Blitz's `hit_inner`. So the page is brought up to
                // date first: a no-op when nothing changed.
                if matches!(
                    event,
                    WindowEvent::PointerMoved { .. }
                        | WindowEvent::PointerButton { .. }
                        | WindowEvent::MouseWheel { .. }
                ) {
                    let time = view.current_animation_time();
                    view.doc.inner_mut().resolve(time);
                }
                view.handle_winit_event(event.clone())
            });
        }
        if let Some(p) = &page {
            guard(|| p.after_event(&event));
        }
        poll(window_id);
        self.settle();
        // What other threads sent meanwhile, and the poll just asked for:
        // winit on Wayland wakes the loop for them only once input stops, so
        // pages set in a few milliseconds waited out a run of typing.
        self.proxy_wake_up(event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        while let Ok(event) = self.blitz.event_queue.try_recv() {
            // A press assistive tech asks for is carried out as a click,
            // through this app's own handling of window events, so the page
            // and the editor see it as they see a hand's.
            // What it asks of the document's text is the editor's.
            if let BlitzShellEvent::Accessibility { window_id, data } = &event
                && let accesskit_xplat::WindowEvent::ActionRequested(request) = &**data
            {
                let edited = self
                    .blitz
                    .windows
                    .get(window_id)
                    .is_some_and(|view| crate::access::action(view, *window_id, request));
                if edited {
                    continue;
                }
                let events = self
                    .blitz
                    .windows
                    .get(window_id)
                    .map(|view| view.accessibility_action_events(request))
                    .unwrap_or_default();
                for e in events {
                    self.window_event(event_loop, *window_id, e);
                }
                continue;
            }
            match event {
                BlitzShellEvent::Embedder(e) => {
                    guard(|| answered(&*e));
                }
                // Blitz's own handling of these, noting which windows'
                // pages changed, for `redraw`.
                BlitzShellEvent::Poll { window_id } => {
                    let start = std::time::Instant::now();
                    if let Some(view) = self.blitz.windows.get_mut(&window_id)
                        && guard(|| view.poll()).unwrap_or(false)
                    {
                        self.frames.entry(window_id).or_default().changed = true;
                    }
                    leaf_dioxus::trace(|| {
                        format!("poll {:.1} ms", start.elapsed().as_secs_f64() * 1000.0)
                    });
                }
                BlitzShellEvent::RequestRedraw { doc_id } => {
                    for (id, view) in &self.blitz.windows {
                        if view.doc.id() == doc_id {
                            self.frames.entry(*id).or_default().changed = true;
                            view.request_redraw();
                        }
                    }
                }
                event => {
                    guard(|| self.blitz.handle_blitz_shell_event(event_loop, event));
                }
            }
        }
        self.drain(event_loop);
        #[cfg(debug_assertions)]
        self.drive(event_loop);
        self.settle();
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        // What other threads sent, whatever woke the loop (see the end of
        // `window_event`).
        self.proxy_wake_up(event_loop);
        self.blitz.about_to_wait(event_loop);
    }

    #[cfg(target_os = "macos")]
    fn macos_handler(
        &mut self,
    ) -> Option<&mut dyn dioxus_native::winit::platform::macos::ApplicationHandlerExtMacOS> {
        Some(self)
    }
}

#[cfg(target_os = "macos")]
impl dioxus_native::winit::platform::macos::ApplicationHandlerExtMacOS for App {
    fn standard_key_binding(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        action: &str,
    ) {
        self.blitz
            .standard_key_binding(event_loop, window_id, action);
    }
}
