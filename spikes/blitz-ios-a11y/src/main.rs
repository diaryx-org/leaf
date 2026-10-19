//! A Blitz page on iOS, read by VoiceOver.
//!
//! One window, made the way leaf's Mac app makes its own (blitz-shell's
//! `BlitzApplication`, wrapped), with three additions for accessibility:
//!
//! - when the window is made, `accesskit_ios`'s `SubclassingAdapter` takes
//!   over winit's `UIView`, so UIKit asks the adapter for the view's
//!   accessibility elements;
//! - after every frame, the adapter is given the page's tree, built by
//!   blitz-dom (a no-op until assistive tech asks);
//! - a request from assistive tech — VoiceOver's double tap — comes back
//!   through the event loop, and a click is carried out as a click at the
//!   middle of the node, through the window's own events.


use accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate,
};
use accesskit_ios::SubclassingAdapter;
use anyrender_vello_cpu::{SoftbufferWindowRenderer, VelloCpuImageRenderer};
use blitz_dom::DocumentConfig;
use blitz_shell::{BlitzApplication, BlitzShellEvent, BlitzShellProxy, WindowConfig};
use blitz_traits::node_id::NodeId;
use dioxus::prelude::*;
use dioxus_native::DioxusDocument;
use dioxus_native::winit::application::ApplicationHandler;
use dioxus_native::winit::dpi::PhysicalPosition;
use dioxus_native::winit::event::{
    ButtonSource, ElementState, MouseButton, PointerSource, WindowEvent,
};
use dioxus_native::winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use dioxus_native::winit::window::{WindowAttributes, WindowId};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const CSS: &str = "
main { padding: 60px 24px; font: 17px system-ui, -apple-system, sans-serif; }
h1 { font-size: 28px; }
button { font-size: 17px; padding: 10px 18px; margin: 12px 0; }
";

#[allow(non_snake_case)]
fn Page() -> Element {
    let mut presses = use_signal(|| 0);
    rsx! {
        style { {CSS} }
        main {
            h1 { "Leaf on Blitz" }
            p { "VoiceOver should read this paragraph, and the button under it." }
            button { onclick: move |_| presses += 1, "Press me" }
            p { role: "status", "Pressed {presses} times" }
        }
    }
}

/// What the adapter's handlers send the event loop: they run inside UIKit's
/// accessibility queries, where the page cannot be touched.
enum Ask {
    /// Assistive tech is listening: build and hand over the tree.
    Tree(WindowId),
    /// Do what assistive tech asked.
    Action(WindowId, ActionRequest),
    /// Activate the element of this label, as VoiceOver's double tap does.
    Activate(String),
}

struct Activation(BlitzShellProxy, WindowId);
impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        eprintln!("accessibility: initial tree requested");
        // The self-test: a few seconds on, activate the button as VoiceOver would.
        if let Ok(label) = std::env::var("BLITZ_A11Y_ACTIVATE") {
            let proxy = self.0.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(4));
                proxy.send_event(BlitzShellEvent::embedder_event(AskBox::new(Ask::Activate(label))));
            });
        }
        self.0
            .send_event(BlitzShellEvent::embedder_event(AskBox::new(Ask::Tree(self.1))));
        None
    }
}

struct Actions(BlitzShellProxy, WindowId);
impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        eprintln!("accessibility action {:?} on {:?}", request.action, request.target_node);
        self.0.send_event(BlitzShellEvent::embedder_event(AskBox::new(Ask::Action(
            self.1, request,
        ))));
    }
}

struct Deactivation;
impl DeactivationHandler for Deactivation {
    fn deactivate_accessibility(&mut self) {}
}

/// `Ask` behind a mutex, as an embedder event must be `Send + Sync`.
struct AskBox(std::sync::Mutex<Option<Ask>>);
impl AskBox {
    fn new(ask: Ask) -> Self {
        Self(std::sync::Mutex::new(Some(ask)))
    }
}
// SAFETY: an `Ask` is made and taken on the main thread only.
unsafe impl Send for AskBox {}
unsafe impl Sync for AskBox {}

/// Vello on the CPU, drawn into a softbuffer surface.
type Renderer = SoftbufferWindowRenderer<VelloCpuImageRenderer>;

struct App {
    blitz: BlitzApplication<Renderer>,
    adapter: Option<SubclassingAdapter>,
    built: bool,
}

impl App {
    /// Hand the adapter the page's tree, if assistive tech is listening.
    fn update_tree(&mut self, id: WindowId) {
        let (Some(adapter), Some(view)) = (self.adapter.as_mut(), self.blitz.windows.get(&id))
        else {
            return;
        };
        let doc = view.doc.inner();
        // blitz-shell lays the page out inside the window's safe area (below
        // the status bar on iOS); the adapter places nodes in the view, which
        // is the whole screen. So the root is moved by the safe area's insets,
        // and every node with it — VoiceOver's hit test too.
        let insets = view.window.safe_area();
        let offset = (insets.left as f64, insets.top as f64);
        let mut built = None;
        let events = adapter.update_if_active(|| {
            let mut tree = doc.build_accessibility_tree();
            let root = tree.tree.as_ref().map(|t| t.root);
            if let Some((_, node)) = tree.nodes.iter_mut().find(|(id, _)| Some(*id) == root) {
                node.set_transform(accesskit::Affine::translate(offset));
            }
            built = Some(tree.nodes.len());
            tree
        });
        if let Some(n) = built {
            eprintln!("accessibility: tree of {n} nodes handed over");
        }
        if let Some(events) = events {
            events.raise();
        }
    }

    fn act(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, request: ActionRequest) {
        if request.action != Action::Click {
            return;
        }
        let Some(view) = self.blitz.windows.get(&id) else {
            return;
        };
        let node = NodeId::from_u64(request.target_node.0);
        let Some(rect) = view.doc.inner().get_client_bounding_rect(node) else {
            return;
        };
        // Pointer events are in the window's surface, and the page is laid
        // out inside its safe area.
        let s = view.window.scale_factor();
        let insets = view.window.safe_area();
        let at = PhysicalPosition::new(
            (rect.x + rect.width / 2.0) * s + insets.left as f64,
            (rect.y + rect.height / 2.0) * s + insets.top as f64,
        );
        for e in click(at) {
            self.window_event(event_loop, id, e);
        }
    }
}

impl App {
    /// Find the element labelled `label` through UIKit's accessibility API,
    /// from winit's view down, and call `accessibilityActivate` on it: what
    /// VoiceOver does on a double tap.
    fn activate(&self, label: &str) {
        use objc2::rc::Retained;
        use objc2::runtime::AnyObject;
        use objc2::msg_send;
        use objc2_foundation::{NSArray, NSString};

        fn find(e: &AnyObject, label: &str, depth: usize) -> Option<Retained<AnyObject>> {
            let l: Option<Retained<NSString>> = unsafe { msg_send![e, accessibilityLabel] };
            if l.is_some_and(|l| l.to_string() == label) {
                return unsafe { Retained::retain(e as *const _ as *mut AnyObject) };
            }
            if depth > 20 {
                return None;
            }
            let kids: Option<Retained<NSArray<AnyObject>>> =
                unsafe { msg_send![e, accessibilityElements] };
            for k in kids.iter().flat_map(|a| a.iter()) {
                if let Some(found) = find(&k, label, depth + 1) {
                    return Some(found);
                }
            }
            None
        }

        for view in self.blitz.windows.values() {
            let Ok(handle) = view.window.window_handle() else { continue };
            let RawWindowHandle::UiKit(h) = handle.as_raw() else { continue };
            let ui_view = unsafe { &*(h.ui_view.as_ptr() as *const AnyObject) };
            match find(ui_view, label, 0) {
                Some(e) => {
                    let ok: bool = unsafe { msg_send![&*e, accessibilityActivate] };
                    eprintln!("self-test: accessibilityActivate on {label:?} -> {ok}");
                }
                None => eprintln!("self-test: no element labelled {label:?}"),
            }
        }
    }
}

fn click(at: PhysicalPosition<f64>) -> Vec<WindowEvent> {
    let button = |pressed: bool| WindowEvent::PointerButton {
        device_id: None,
        state: if pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        },
        position: at,
        primary: true,
        button: ButtonSource::Mouse(MouseButton::Left),
        is_macos_activation_click: false,
    };
    vec![
        WindowEvent::PointerMoved {
            device_id: None,
            position: at,
            primary: true,
            source: PointerSource::Mouse,
        },
        button(true),
        button(false),
    ]
}

impl ApplicationHandler for App {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.can_create_surfaces(event_loop);
        if self.built {
            return;
        }
        self.built = true;
        let proxy = self.blitz.proxy.clone();
        for (id, view) in self.blitz.windows.iter_mut() {
            view.downcast_doc_mut::<DioxusDocument>().initial_build();
            view.request_redraw();
            let Ok(handle) = view.window.window_handle() else {
                continue;
            };
            let RawWindowHandle::UiKit(h) = handle.as_raw() else {
                continue;
            };
            // SAFETY: winit's view, alive for as long as its window.
            self.adapter = Some(unsafe {
                SubclassingAdapter::new(
                    h.ui_view.as_ptr(),
                    Activation(proxy.clone(), *id),
                    Actions(proxy.clone(), *id),
                    Deactivation,
                )
            });
            eprintln!("accessibility adapter on {:?}", h.ui_view);
        }
    }

    fn destroy_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.destroy_surfaces(event_loop);
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let redraw = matches!(event, WindowEvent::RedrawRequested);
        self.blitz.window_event(event_loop, id, event);
        if redraw {
            self.update_tree(id);
        }
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        while let Ok(event) = self.blitz.event_queue.try_recv() {
            if let BlitzShellEvent::Embedder(data) = &event {
                let ask = data
                    .downcast_ref::<AskBox>()
                    .and_then(|b| b.0.lock().unwrap().take());
                match ask {
                    Some(Ask::Tree(id)) => self.update_tree(id),
                    Some(Ask::Action(id, request)) => self.act(event_loop, id, request),
                    Some(Ask::Activate(label)) => self.activate(&label),
                    None => {}
                }
                continue;
            }
            self.blitz.handle_blitz_shell_event(event_loop, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.blitz.about_to_wait(event_loop);
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("an event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let (proxy, queue) = BlitzShellProxy::new(event_loop.create_proxy());
    let mut blitz = BlitzApplication::new(proxy, queue);
    let document = DioxusDocument::new(VirtualDom::new(Page), DocumentConfig::default());
    let attributes = WindowAttributes::default().with_title("BlitzA11y");
    blitz.add_window(WindowConfig::with_attributes(
        Box::new(document),
        Renderer::new(),
        attributes,
    ));
    event_loop
        .run_app(App {
            blitz,
            adapter: None,
            built: false,
        })
        .expect("the event loop");
}
