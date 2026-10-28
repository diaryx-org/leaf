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
//! A finger that goes down on one of a selection's grab handles (`row.rs`
//! draws them) drags that end of the selection instead, as a phone's text
//! view does: winit's pointer events again, since Blitz sends no mouse event
//! for a finger that moves.
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

/// How far from a grab handle's knob, in CSS pixels, a finger still takes
/// it: the 44-point target a phone gives a control, for a knob of ten.
const REACH: f64 = 22.0;

/// A selection's end a finger has taken by its handle: the end that stays,
/// whether the one dragged is the end (rather than the start), and how far
/// below the middle of the dragged end's line the finger went down — kept
/// as it moves, so the end follows the line the finger is over and not the
/// knob under it.
#[derive(Clone, Copy, Debug)]
pub struct Grip {
    fixed: usize,
    end: bool,
    dy: f64,
}

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
    /// Went down on a grab handle: it drags the selection's end.
    grip: Option<Grip>,
}

impl Editor {
    /// A finger went down (`pressed`) or came up, at `at` in physical pixels.
    pub(crate) fn touch_button(&self, pressed: bool, at: PhysicalPosition<f64>) {
        let st = &*self.st;
        let (x, y) = self.logical(at);
        if pressed {
            // A finger on a gliding page stops it where it is.
            st.caught.set(self.gliding());
            self.stop_glide();
            st.tracker.borrow_mut().reset();
            st.tracker.borrow_mut().push(crate::clock::ms(), y);
            let grip = st.touch_mode.get().then(|| self.grip_at(x, y)).flatten();
            st.touch_mode.set(true);
            st.gripped.set(grip.is_some());
            st.touch.set(Some(Touch {
                x,
                y,
                top: self.top(),
                scrolling: false,
                turning: false,
                grip,
            }));
            if grip.is_some() {
                trace(|| format!("grip {grip:?}"));
                // The host hides what it shows over the selection while it
                // is dragged ([`Editor::dragging_grip`]).
                self.repaint();
            }
            return;
        }
        // A handle let go: the selection stands, and the host shows its menu
        // over it again.
        if st.touch.get().is_some_and(|t| t.grip.is_some()) {
            st.touch.set(None);
            self.repaint();
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
        // A scroll let go: the page carries on at the finger's speed, or
        // springs back from past an end.
        if st.touch.get().is_some_and(|t| t.scrolling) {
            st.tracker.borrow_mut().push(crate::clock::ms(), y);
            let vel = -st.tracker.borrow().velocity(crate::clock::ms());
            st.touch.set(None);
            self.start_glide(vel);
            return;
        }
        st.touch.set(None);
    }

    /// A finger moved, to `at` in physical pixels.
    pub(crate) fn touch_moved(&self, at: PhysicalPosition<f64>) {
        let st = &*self.st;
        let Some(mut t) = st.touch.get() else { return };
        let (x, y) = self.logical(at);
        if let Some(g) = t.grip {
            self.drag_grip(g, x, y);
            return;
        }
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
            // Past either end the page stretches, giving less the further
            // it is pulled.
            let raw = t.top - dy;
            let max = self.max_top().max(0.0);
            self.scroll_to(unit::scroll::rubber(raw, 0.0, max, self.bounds().height));
            st.tracker.borrow_mut().push(crate::clock::ms(), y);
        }
        st.touch.set(Some(t));
    }

    /// Whether the page is moving on its own.
    fn gliding(&self) -> bool {
        self.st.glide.get() % 2 == 1
    }

    /// Stop the page where it is: a finger landed, or a host's wheel
    /// started to scroll again.
    pub fn stop_glide(&self) {
        let g = self.st.glide.get();
        if g % 2 == 1 {
            self.st.glide.set(g + 1);
        }
    }

    /// Let the page carry on at `vel` CSS pixels a millisecond, slowing as a
    /// phone's scroll view slows, and spring back if it is past an end —
    /// frame by frame, each moved as far as the time since the last says.
    /// An odd `glide` is a glide under way; landing a finger, or starting
    /// another, makes it even and the loop below stops. A finger's throw
    /// starts one; so does a host's, for a touchpad's fingers lifting from
    /// a scroll where the system gives it no momentum of its own.
    pub fn start_glide(&self, vel: f64) {
        let st = &*self.st;
        let max = self.max_top().max(0.0);
        let top = self.top();
        let still = vel.abs() < 0.05;
        if still && (0.0..=max).contains(&top) {
            return;
        }
        let Some((rt, scope)) = st.home.clone() else {
            self.scroll_to(top.clamp(0.0, max));
            return;
        };
        let id = st.glide.get() / 2 * 2 + 3;
        st.glide.set(id);
        let ed = self.clone();
        let mut g = unit::scroll::Glide {
            pos: top,
            vel: if still { 0.0 } else { vel },
            min: 0.0,
            max,
        };
        rt.spawn(scope, async move {
            let mut last = crate::clock::Instant::now();
            loop {
                crate::clock::sleep(std::time::Duration::from_millis(8)).await;
                if ed.st.glide.get() != id {
                    return;
                }
                let now = crate::clock::Instant::now();
                let dt = now.duration_since(last).as_secs_f64() * 1000.0;
                last = now;
                g.max = ed.max_top().max(0.0);
                let moving = g.step(dt);
                ed.scroll_to(g.pos);
                // A change made from a task waits, on iOS, for the next event
                // to be drawn: ask for the frame.
                ed.st.window.request_redraw();
                if !moving {
                    ed.st.glide.set(id + 1);
                    return;
                }
            }
        });
    }

    /// Whether a finger is dragging one of the selection's grab handles: a
    /// host hides an edit menu over the selection until it lets go, as a
    /// phone's text view does.
    pub fn dragging_grip(&self) -> bool {
        self.st.touch.get().is_some_and(|t| t.grip.is_some())
    }

    /// The grab handle within reach of (`x`, `y`), in CSS pixels from the
    /// window's top-left, if the selection has one there: the knob over the
    /// start, or under the end, and the stem between. The nearer, where both
    /// are in reach.
    fn grip_at(&self, x: f64, y: f64) -> Option<Grip> {
        let (lo, hi) = self.st.doc.borrow().selection()?;
        let reach = |at: usize, end: bool| -> Option<(f64, Grip)> {
            let (gx, gy, _, h) = self.rect_of(at, at)?;
            // The knob's centre: five pixels past the line's edge, where
            // `.grip` hangs it.
            let ky = if end { gy + h + 5.0 } else { gy - 5.0 };
            let along = (x - gx).abs();
            let d = if y >= gy && y <= gy + h {
                along
            } else {
                along.hypot(y - ky)
            };
            (d <= REACH).then_some((
                d,
                Grip {
                    fixed: if end { lo } else { hi },
                    end,
                    dy: y - (gy + h / 2.0),
                },
            ))
        };
        match (reach(lo, false), reach(hi, true)) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a.1 } else { b.1 }),
            (a, b) => a.or(b).map(|(_, g)| g),
        }
    }

    /// A grab handle dragged to (`x`, `y`): its end of the selection to the
    /// character under the line the finger is over, never past the other
    /// end — a selection a handle drags is never empty, and its ends never
    /// swap.
    fn drag_grip(&self, g: Grip, x: f64, y: f64) {
        let Some(at) = self.offset_at_point(x, y - g.dy) else {
            return;
        };
        {
            let mut doc = self.st.doc.borrow_mut();
            let was = (doc.anchor, doc.caret);
            // From the end that stays, so a hit-test that snaps to a caret
            // stop extends the way a drag-select would.
            doc.select_range(g.fixed, g.fixed);
            doc.place_caret(at, true);
            let kept = doc
                .selection()
                .is_some_and(|(lo, hi)| if g.end { lo == g.fixed } else { hi == g.fixed });
            if !kept {
                (doc.anchor, doc.caret) = was;
            }
            if (doc.anchor, doc.caret) == was {
                return;
            }
        }
        self.repaint();
    }

    /// A position from winit, in CSS pixels.
    fn logical(&self, at: PhysicalPosition<f64>) -> (f64, f64) {
        let scale = self.st.window.scale_factor();
        (at.x / scale, at.y / scale)
    }
}
