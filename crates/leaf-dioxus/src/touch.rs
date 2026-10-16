//! A finger: a tap, a scroll, or a sideways turn of the page.
//!
//! Blitz sends a touch no `mousedown`. A finger produces pointer events and,
//! if it did not pan, a `click` — but none of the mouse events the rows
//! listen for. So under a finger the rows are heard through `click`
//! ([`Pointer::Tap`](crate::Pointer::Tap)), which is exactly a tap, and a
//! drag never selects: it scrolls the editor, read here from winit's pointer
//! events, or — sideways, where the host allows it ([`Editor::set_turns`]) —
//! slides the page and turns it ([`Host::turn`](crate::Host::turn)).
//!
//! Found in Diaryx's iOS port, where the editor first ran under a finger.

use dioxus_native::winit::dpi::PhysicalPosition;

use crate::editor::{Editor, set};
use crate::trace::trace;

/// Whether allowing the input method raises an on-screen keyboard, so that
/// the editor waits for a tap on the page to ask for it rather than asking
/// on mount, as it does where the keyboard is a real one.
pub const KEYBOARD_ON_TAP: bool = cfg!(any(target_os = "ios", target_os = "android"));

/// A finger that moves further than this, in CSS pixels, is a scroll and not
/// a tap: the caret stays where it was.
const SLOP: f64 = 8.0;

/// A finger that went down on the editor and has not yet come up: where it
/// went down, the scroll offset then, and what it has turned out to be.
#[derive(Clone, Copy, Debug)]
pub struct Touch {
    x: f64,
    y: f64,
    top: f64,
    scrolling: bool,
    /// Dragged sideways first: the page is being turned, not scrolled.
    turning: bool,
}

impl Editor {
    /// A finger went down (`pressed`) or came up, at `at` in physical pixels.
    pub(crate) fn touch_button(&self, pressed: bool, at: PhysicalPosition<f64>) {
        let st = &*self.st;
        let (x, y) = self.logical(at);
        if pressed {
            st.touch_mode.set(true);
            st.touch.set(Some(Touch {
                x,
                y,
                top: self.top(),
                scrolling: false,
                turning: false,
            }));
            return;
        }
        // A turn let go: far enough is a turn, short of it a change of mind,
        // and the page springs back either way.
        if let Some(t) = st.touch.get().filter(|t| t.turning) {
            let dx = x - t.x;
            set(self.sig.slide, 0.0);
            if dx.abs() > self.bounds().width * 0.25 {
                let way = if dx < 0.0 { 1 } else { -1 };
                trace(|| format!("turn {way}"));
                st.touch.set(None);
                st.host.turn(self, way);
                return;
            }
        }
        st.touch.set(None);
    }

    /// A finger moved, to `at` in physical pixels.
    pub(crate) fn touch_moved(&self, at: PhysicalPosition<f64>) {
        let st = &*self.st;
        let Some(mut t) = st.touch.get() else { return };
        let (x, y) = self.logical(at);
        let (dx, dy) = (x - t.x, y - t.y);
        // Which way the finger first went decides: sideways turns the page,
        // where it can be turned that way and nothing is selected; anything
        // else scrolls.
        if !t.scrolling && !t.turning {
            let (back, forward) = st.turns.get();
            let can = if dx < 0.0 { forward } else { back };
            // A drag from the very edge is the system's Back, not a turn.
            let from_edge = t.x - self.bounds().left < 24.0;
            let selected = st.doc.borrow().selection().is_some();
            if dx.abs() > SLOP * 2.0 && dx.abs() > dy.abs() * 2.0 && can && !from_edge && !selected
            {
                t.turning = true;
            } else if dy.abs() > SLOP {
                t.scrolling = true;
            }
        }
        if t.turning {
            set(self.sig.slide, dx);
        }
        if t.scrolling {
            self.scroll_to((t.top - dy).clamp(0.0, self.max_top()));
        }
        st.touch.set(Some(t));
    }

    /// A position from winit, in CSS pixels.
    fn logical(&self, at: PhysicalPosition<f64>) -> (f64, f64) {
        let scale = self.st.window.scale_factor();
        (at.x / scale, at.y / scale)
    }
}
