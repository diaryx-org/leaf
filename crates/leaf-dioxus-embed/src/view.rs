//! One editor in one host view: what dioxus-native's `launch` and blitz-shell's
//! `View::init` put together around a winit window, put together here around
//! a [`HostedWindow`].
//!
//! A [`Hosted`] owns the Blitz view (the document, its renderer and the
//! pointer state Blitz keeps), the Dioxus runtime the editor's signals live
//! in, and the editor itself once its component has made it. Every host call
//! comes in as a method here; each takes the view's borrow only for as long as
//! Blitz needs it, so a host callback that calls back in never finds it held.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use blitz_dom::DocumentConfig;
use blitz_shell::{BlitzShellEvent, BlitzShellProvider, View};
use blitz_traits::events::MouseEventButtons;
use blitz_traits::shell::{ColorScheme, Viewport};
use dioxus::prelude::*;
use dioxus_core::{Runtime, ScopeId, VirtualDom};
use dioxus_native::DioxusDocument;
use dioxus_native::winit::dpi::PhysicalPosition;
use dioxus_native::winit::event::{
    ButtonSource, ElementState, Ime, MouseButton, MouseScrollDelta, PointerKind, PointerSource,
    TouchPhase, WindowEvent,
};
use dioxus_native::winit::keyboard::ModifiersState;
use dioxus_native::winit::window::{Theme, Window, WindowId};

use crate::renderer::Renderer;
use leaf_core::Doc;
use leaf_dioxus::{Editor, Host, Look, use_editor};

use crate::callbacks::{Callbacks, Scroll};
use crate::guard::guard;
use crate::host::SwiftHost;
use crate::runtime;
use crate::window::HostedWindow;

/// What the root component is made with, taken on its first render.
#[derive(Clone)]
pub(crate) struct Init {
    pub doc: Rc<RefCell<Option<Doc>>>,
    pub host: Rc<dyn Host>,
    /// Where the component leaves the editor it made, for the host's calls.
    pub editor: Rc<RefCell<Option<Editor>>>,
    pub look: Look,
    /// Laid out for paper (`pdf.rs`): no caret, no status line.
    pub print: bool,
}

/// The page: the editor, and nothing else — the host draws its chrome
/// natively around the view.
#[allow(non_snake_case)]
pub(crate) fn Root() -> Element {
    let init = use_context::<Init>();
    let ed = use_editor(
        || {
            init.doc
                .borrow_mut()
                .take()
                .unwrap_or_else(|| Doc::blank().expect("a blank document"))
        },
        || init.look.clone(),
        || init.host.clone(),
    );
    use_hook(|| *init.editor.borrow_mut() = Some(ed.clone()));
    rsx! {
        {ed.render()}
        if init.print {
            style { ".caret-in, .status {{ display: none; }}" }
        }
    }
}

pub struct Hosted {
    window: Arc<HostedWindow>,
    view: RefCell<View<Renderer>>,
    runtime: Rc<Runtime>,
    editor: Rc<RefCell<Option<Editor>>>,
    mods: Cell<ModifiersState>,
    /// The scroll as the host last heard it, from the `scrolled` callback or
    /// its own `set_scroll`: a change the editor makes is reported against
    /// it, and a move of the host's never comes back to it.
    scroll: Cell<Scroll>,
    /// Whether a scroll report is already on its way.
    scroll_queued: Cell<bool>,
}

/// Modifier bits as the ABI spells them (`LEAF_MOD_*`).
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CONTROL: u32 = 2;
pub const MOD_ALT: u32 = 4;
pub const MOD_COMMAND: u32 = 8;

pub fn winit_mods(bits: u32) -> ModifiersState {
    let mut m = ModifiersState::empty();
    m.set(ModifiersState::SHIFT, bits & MOD_SHIFT != 0);
    m.set(ModifiersState::CONTROL, bits & MOD_CONTROL != 0);
    m.set(ModifiersState::ALT, bits & MOD_ALT != 0);
    m.set(ModifiersState::META, bits & MOD_COMMAND != 0);
    m
}

pub fn dioxus_mods(bits: u32) -> Modifiers {
    let mut m = Modifiers::empty();
    m.set(Modifiers::SHIFT, bits & MOD_SHIFT != 0);
    m.set(Modifiers::CONTROL, bits & MOD_CONTROL != 0);
    m.set(Modifiers::ALT, bits & MOD_ALT != 0);
    // ⌘, as winit reports it and the editor's bindings match it.
    m.set(Modifiers::SUPER, bits & MOD_COMMAND != 0);
    m
}

impl Hosted {
    /// The editor on `doc` in the host's view `view`.
    pub fn new(
        view: NonNull<c_void>,
        (width, height, scale): (f64, f64, f64),
        dark: bool,
        doc: Doc,
        callbacks: Callbacks,
    ) -> Rc<Self> {
        let proxy = runtime::proxy();
        let window = Arc::new(HostedWindow::new(
            view, width, height, scale, dark, callbacks,
        ));
        let as_dyn: Arc<dyn Window> = window.clone();
        let editor = Rc::new(RefCell::new(None));

        let mut vdom = VirtualDom::new(Root);
        let host: Rc<dyn Host> = Rc::new(SwiftHost::new(window.id()));
        vdom.insert_any_root_context(Box::new(Init {
            doc: Rc::new(RefCell::new(Some(doc))),
            host,
            editor: editor.clone(),
            look: Look::default(),
            print: false,
        }));
        let net = Arc::new(blitz_net::Provider::new(Some(Arc::new(proxy.clone()) as _)));
        vdom.provide_root_context(Arc::clone(&net));
        let mut document = DioxusDocument::new(
            vdom,
            DocumentConfig {
                net_provider: Some(net as _),
                ..Default::default()
            },
        );
        let runtime = document.vdom.runtime();

        // What dioxus-native provides every window's root, but for its
        // window-event handlers (the host sends events itself) and its head
        // elements (the editor sets no title).
        let renderer = Renderer::new();
        {
            let (w, r) = (as_dyn.clone(), renderer.clone());
            document.vdom.in_scope(ScopeId::ROOT, move || {
                provide_context(w);
                provide_context(r);
            });
        }

        // blitz-shell's `View::init`, but for making the window.
        let theme = window.theme().unwrap_or(Theme::Light);
        let size = window.surface_size();
        let viewport = Viewport::new(
            size.width,
            size.height,
            scale as f32,
            match theme {
                Theme::Dark => ColorScheme::Dark,
                Theme::Light => ColorScheme::Light,
            },
        );
        {
            let mut inner = document.inner.borrow_mut();
            inner.set_viewport(viewport);
            inner.set_shell_provider(Arc::new(BlitzShellProvider::new(
                as_dyn.clone(),
                proxy.clone(),
            )));
        }
        document.initial_build();

        let blitz = View {
            doc: Box::new(document),
            renderer,
            waker: None,
            proxy: proxy.clone(),
            window: as_dyn,
            theme_override: None,
            keyboard_modifiers: Default::default(),
            buttons: MouseEventButtons::None,
            pointer_pos: Default::default(),
            active_events: Default::default(),
            animation_timer: None,
            is_visible: true,
            safe_area_insets: Default::default(),
        };
        let hosted = Rc::new(Self {
            window,
            view: RefCell::new(blitz),
            runtime,
            editor,
            mods: Cell::new(ModifiersState::empty()),
            scroll: Cell::default(),
            scroll_queued: Cell::new(false),
        });
        runtime::insert(hosted.id(), hosted.clone());
        // On a Mac the renderer is ready at once and says so down the
        // channel; `complete_resume` paints the first frame.
        hosted.view.borrow_mut().resume();
        hosted
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn doc_id(&self) -> Option<usize> {
        self.view.try_borrow().ok().map(|v| v.doc.id())
    }

    pub fn callbacks(&self) -> &Callbacks {
        self.window.callbacks()
    }

    /// Run `f` on the editor, inside its Dioxus runtime — where its signals
    /// can be written and tasks spawned — then let the page catch up.
    pub fn with_editor<R>(&self, f: impl FnOnce(&Editor) -> R) -> Option<R> {
        let ed = self.editor.borrow().clone()?;
        let r = guard(|| self.runtime.in_scope(ScopeId::ROOT, || f(&ed)));
        self.wake();
        r
    }

    /// Ask for a poll of the page on the next turn.
    fn wake(&self) {
        runtime::proxy().send_event(BlitzShellEvent::Poll {
            window_id: self.id(),
        });
    }

    pub fn poll(&self) {
        if let Ok(mut v) = self.view.try_borrow_mut() {
            guard(|| v.poll());
        } else {
            self.wake();
        }
        self.report_scroll();
    }

    /// Draw a frame; `false` if the view was busy.
    pub fn redraw(&self) -> bool {
        match self.view.try_borrow_mut() {
            Ok(mut v) => {
                guard(|| v.redraw());
                drop(v);
                self.report_scroll();
                true
            }
            Err(_) => false,
        }
    }

    /// The host scrolled the editor to `top`.
    pub fn set_scroll(&self, top: f64) {
        self.scroll.set(Scroll {
            top,
            ..self.scroll.get()
        });
        self.with_editor(|ed| ed.scroll_to(top));
    }

    /// The editor's scroll, read without the wake-up `with_editor` sends.
    fn current_scroll(&self) -> Option<Scroll> {
        let ed = self.editor.borrow().clone()?;
        guard(|| {
            self.runtime.in_scope(ScopeId::ROOT, || Scroll {
                top: ed.top(),
                height: ed.scroll_height(),
                inset_top: ed.bar_height(),
                inset_bottom: leaf_dioxus::STATUS,
            })
        })
    }

    /// Tell the host, on the next turn, if the editor's scroll is not what
    /// it last heard: after a poll or a frame, which every change to the
    /// viewport or the content's height goes through. Read again then, so a
    /// move the host makes in between is not undone by a stale report.
    fn report_scroll(&self) {
        if self.scroll_queued.get() || self.current_scroll() == Some(self.scroll.get()) {
            return;
        }
        self.scroll_queued.set(true);
        let id = self.id();
        runtime::later(move || {
            let Some(h) = runtime::get(id) else { return };
            h.scroll_queued.set(false);
            if let Some(now) = h.current_scroll()
                && now != h.scroll.get()
            {
                h.scroll.set(now);
                h.callbacks().scrolled(now);
            }
        });
    }

    pub fn complete_resume(&self) {
        if let Ok(mut v) = self.view.try_borrow_mut() {
            guard(|| v.complete_resume());
        }
    }

    /// An event for Blitz and then for the editor, which reads some of the
    /// same ones (its size, the input method, a click's release).
    fn dispatch(&self, event: WindowEvent) {
        if let Ok(mut v) = self.view.try_borrow_mut() {
            guard(|| {
                // A change a poll made to the page is laid out at the next
                // redraw, and a pointer event the host sends before it would
                // hit-test the tree as it was, through nodes the change
                // removed — a panic in Blitz's `hit_inner`. blitz-shell's
                // loop rarely lets one in between; AppKit's main queue does.
                // So the page is brought up to date first: a no-op when
                // nothing changed. (`has_changes` would say, but it answers
                // the opposite of its name in this Blitz.)
                if matches!(
                    event,
                    WindowEvent::PointerMoved { .. }
                        | WindowEvent::PointerButton { .. }
                        | WindowEvent::MouseWheel { .. }
                ) {
                    let time = v.current_animation_time();
                    v.doc.inner_mut().resolve(time);
                }
                v.handle_winit_event(event.clone())
            });
        }
        self.with_editor(|ed| ed.window_event(&event));
    }

    pub fn resize(&self, width: f64, height: f64, scale: f64) {
        let old = self.window.scale_factor();
        let size = self.window.set_geometry(width, height, scale);
        if old != scale
            && let Ok(mut v) = self.view.try_borrow_mut()
        {
            v.with_viewport(|vp| vp.set_hidpi_scale(scale as f32));
        }
        self.dispatch(WindowEvent::SurfaceResized(size));
    }

    pub fn set_dark(&self, dark: bool) {
        let theme = self.window.set_theme_dark(dark);
        self.dispatch(WindowEvent::ThemeChanged(theme));
    }

    pub fn set_focused(&self, yes: bool) {
        self.window.set_focused(yes);
        self.dispatch(WindowEvent::Focused(yes));
    }

    /// The modifiers held, which Blitz reads off each pointer event.
    pub fn modifiers(&self, bits: u32) {
        let m = winit_mods(bits);
        if self.mods.replace(m) != m {
            self.dispatch(WindowEvent::ModifiersChanged(m.into()));
        }
    }

    fn at(&self, x: f64, y: f64) -> PhysicalPosition<f64> {
        let s = self.window.scale_factor();
        PhysicalPosition::new(x * s, y * s)
    }

    /// The pointer moved to (`x`, `y`) points, a button down or not.
    pub fn pointer_moved(&self, x: f64, y: f64) {
        self.dispatch(WindowEvent::PointerMoved {
            device_id: None,
            position: self.at(x, y),
            primary: true,
            source: PointerSource::Mouse,
        });
    }

    /// A button pressed or released at (`x`, `y`): 0 the main button, 1 the
    /// secondary.
    pub fn pointer_button(&self, x: f64, y: f64, button: u32, pressed: bool) {
        let button = match button {
            1 => MouseButton::Right,
            2 => MouseButton::Middle,
            _ => MouseButton::Left,
        };
        self.dispatch(WindowEvent::PointerButton {
            device_id: None,
            state: if pressed {
                ElementState::Pressed
            } else {
                ElementState::Released
            },
            position: self.at(x, y),
            primary: true,
            button: ButtonSource::Mouse(button),
            is_macos_activation_click: false,
        });
    }

    pub fn pointer_left(&self) {
        self.dispatch(WindowEvent::PointerLeft {
            device_id: None,
            position: None,
            primary: true,
            kind: PointerKind::Mouse,
        });
    }

    /// The wheel or the trackpad, in points (`precise`) or lines.
    pub fn scroll(&self, dx: f64, dy: f64, precise: bool) {
        let delta = if precise {
            let s = self.window.scale_factor();
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(dx * s, dy * s))
        } else {
            MouseScrollDelta::LineDelta(dx as f32, dy as f32)
        };
        self.dispatch(WindowEvent::MouseWheel {
            device_id: None,
            delta,
            phase: TouchPhase::Moved,
        });
    }

    /// Text from the host's input client: typed, committed by an input
    /// method, picked from the Character Viewer, dictated.
    pub fn insert_text(&self, text: String) {
        self.dispatch(WindowEvent::Ime(Ime::Commit(text)));
    }

    /// An input method's composition so far, with its selection in bytes.
    pub fn set_marked(&self, text: String, sel: Option<(usize, usize)>) {
        self.dispatch(WindowEvent::Ime(Ime::Preedit(text, sel)));
    }

    /// Delete around the caret before the next insertion: press-and-hold
    /// replacing the letter it offered accents for.
    pub fn delete_surrounding(&self, before: usize, after: usize) {
        self.dispatch(WindowEvent::Ime(Ime::DeleteSurrounding {
            before_bytes: before,
            after_bytes: after,
        }));
    }
}

impl Drop for Hosted {
    fn drop(&mut self) {
        // Drop the surface before the host's view goes.
        if let Ok(mut v) = self.view.try_borrow_mut() {
            v.suspend();
        }
    }
}
