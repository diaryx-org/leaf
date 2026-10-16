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
use dioxus_native::winit::dpi::PhysicalPosition;
use dioxus_native::winit::event::{
    ButtonSource, ElementState, Ime, MouseButton, MouseScrollDelta, PointerKind, PointerSource,
    TouchPhase, WindowEvent,
};
use dioxus_native::winit::keyboard::ModifiersState;
use dioxus_native::winit::window::{Theme, Window, WindowId};
use dioxus_native::{DioxusDocument, DioxusNativeWindowRenderer};
use leaf_core::Doc;
use leaf_dioxus::{Editor, Host, Look, use_editor};

use crate::callbacks::Callbacks;
use crate::host::SwiftHost;
use crate::runtime;
use crate::window::HostedWindow;

/// What the root component is made with, taken on its first render.
#[derive(Clone)]
struct Init {
    doc: Rc<RefCell<Option<Doc>>>,
    host: Rc<dyn Host>,
    /// Where the component leaves the editor it made, for the host's calls.
    editor: Rc<RefCell<Option<Editor>>>,
}

/// The page: the editor, and nothing else — the host draws its chrome
/// natively around the view.
#[allow(non_snake_case)]
fn Root() -> Element {
    let init = use_context::<Init>();
    let ed = use_editor(
        || {
            init.doc
                .borrow_mut()
                .take()
                .unwrap_or_else(|| Doc::blank().expect("a blank document"))
        },
        Look::default,
        || init.host.clone(),
    );
    use_hook(|| *init.editor.borrow_mut() = Some(ed.clone()));
    ed.render()
}

pub struct Hosted {
    window: Arc<HostedWindow>,
    view: RefCell<View<DioxusNativeWindowRenderer>>,
    runtime: Rc<Runtime>,
    editor: Rc<RefCell<Option<Editor>>>,
    mods: Cell<ModifiersState>,
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
        let renderer = DioxusNativeWindowRenderer::new();
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
        let r = self.runtime.in_scope(ScopeId::ROOT, || f(&ed));
        self.wake();
        Some(r)
    }

    /// Ask for a poll of the page on the next turn.
    fn wake(&self) {
        runtime::proxy().send_event(BlitzShellEvent::Poll {
            window_id: self.id(),
        });
    }

    pub fn poll(&self) {
        if let Ok(mut v) = self.view.try_borrow_mut() {
            v.poll();
        } else {
            self.wake();
        }
    }

    /// Draw a frame; `false` if the view was busy.
    pub fn redraw(&self) -> bool {
        match self.view.try_borrow_mut() {
            Ok(mut v) => {
                v.redraw();
                true
            }
            Err(_) => false,
        }
    }

    pub fn complete_resume(&self) {
        if let Ok(mut v) = self.view.try_borrow_mut() {
            v.complete_resume();
        }
    }

    /// An event for Blitz and then for the editor, which reads some of the
    /// same ones (its size, the input method, a click's release).
    fn dispatch(&self, event: WindowEvent) {
        if let Ok(mut v) = self.view.try_borrow_mut() {
            v.handle_winit_event(event.clone());
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
