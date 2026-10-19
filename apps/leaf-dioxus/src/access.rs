//! What assistive tech asks a window to do — VoiceOver's press, an agent's
//! `AXPress` — which Blitz hears and leaves to its host (blitz-shell's
//! handler for it is a `TODO`).
//!
//! A click is carried out as a click: the pointer pressed and released at the
//! middle of the node's box, through the window's own event handling, so the
//! node acts on it exactly as on a click by hand.

use accesskit::{Action, ActionRequest};
use anyrender::WindowRenderer;
use blitz_shell::View;
use blitz_traits::node_id::NodeId;
use dioxus_native::winit::dpi::PhysicalPosition;
use dioxus_native::winit::event::{
    ButtonSource, ElementState, MouseButton, PointerSource, WindowEvent,
};

/// The window events that carry out `request` in `view`, if it is one this
/// app carries out.
pub fn events<R: WindowRenderer>(view: &View<R>, request: &ActionRequest) -> Vec<WindowEvent> {
    match request.action {
        Action::Click => {
            let node = NodeId::from_u64(request.target_node.0);
            let Some(rect) = view.doc.inner().get_client_bounding_rect(node) else {
                return Vec::new();
            };
            let s = view.window.scale_factor();
            let at = PhysicalPosition::new(
                (rect.x + rect.width / 2.0) * s,
                (rect.y + rect.height / 2.0) * s,
            );
            click(at)
        }
        _ => Vec::new(),
    }
}

/// A primary click at `at`, physical pixels: moved there, pressed, released.
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
