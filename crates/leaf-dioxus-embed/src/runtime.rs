//! The run loop that is not there: Blitz's wake-ups as main-queue blocks.
//!
//! dioxus-native runs a winit event loop and wakes it through an
//! `EventLoopProxy`; Blitz's shell sends its events (poll this document,
//! redraw that one, the renderer is ready) down a channel and wakes the loop
//! to read them. Here the host's run loop is the only one, so the proxy's
//! wake-up is a block on the main dispatch queue that reads the channel, and a
//! redraw is another, coalesced so a burst of requests draws once.
//!
//! Everything is thread-local to the main thread: the views, the channel, and
//! the tokio runtime Blitz's network provider and the editor's timers run on,
//! entered once and never left.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use blitz_shell::{BlitzShellEvent, BlitzShellProxy};
use dioxus_native::winit::event_loop::{EventLoopProxy, EventLoopProxyProvider};
use dioxus_native::winit::window::WindowId;

use crate::view::Hosted;

struct Runtime {
    proxy: BlitzShellProxy,
    queue: Receiver<BlitzShellEvent>,
}

thread_local! {
    static RUNTIME: OnceCell<Runtime> = const { OnceCell::new() };
    static VIEWS: RefCell<HashMap<WindowId, Rc<Hosted>>> = RefCell::default();
    static REDRAWS: RefCell<HashSet<WindowId>> = RefCell::default();
}

/// Whether a drain of the channel is already queued.
static WAKE_QUEUED: AtomicBool = AtomicBool::new(false);
/// Whether a redraw is already queued.
static REDRAW_QUEUED: AtomicBool = AtomicBool::new(false);

/// The proxy's wake-up: read the channel on the main queue's next turn.
#[derive(Debug)]
struct MainQueue;

impl EventLoopProxyProvider for MainQueue {
    fn wake_up(&self) {
        if !WAKE_QUEUED.swap(true, Ordering::AcqRel) {
            on_main(drain);
        }
    }
}

/// Run `f` on the main queue's next turn.
pub fn on_main(f: fn()) {
    #[cfg(target_vendor = "apple")]
    dispatch2::DispatchQueue::main().exec_async(f);
    #[cfg(not(target_vendor = "apple"))]
    f();
}

/// Run a closure on the main queue's next turn. It is only ever run there,
/// on the thread that made it, so it need not be `Send`.
pub fn later(f: impl FnOnce() + 'static) {
    struct OnMain(Box<dyn FnOnce()>);
    // SAFETY: made on the main thread, run on the main queue, which is the
    // main thread.
    unsafe impl Send for OnMain {}
    let job = OnMain(Box::new(f));
    #[cfg(target_vendor = "apple")]
    dispatch2::DispatchQueue::main().exec_async(move || {
        let job = job;
        (job.0)()
    });
    #[cfg(not(target_vendor = "apple"))]
    (job.0)();
}

/// The shell's proxy, made on first use with the tokio runtime entered.
pub fn proxy() -> BlitzShellProxy {
    RUNTIME.with(|r| {
        r.get_or_init(|| {
            crate::guard::install_hook();
            // Blitz's net provider and the editor's timers need a tokio
            // context wherever they are made or polled, which here is
            // anywhere on the main thread: enter it for good.
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime");
            std::mem::forget(rt.enter());
            std::mem::forget(rt);
            let winit = EventLoopProxy::new(Arc::new(MainQueue));
            let (proxy, queue) = BlitzShellProxy::new(winit);
            Runtime { proxy, queue }
        })
        .proxy
        .clone()
    })
}

pub fn insert(id: WindowId, hosted: Rc<Hosted>) {
    VIEWS.with(|v| v.borrow_mut().insert(id, hosted));
}

pub fn remove(id: WindowId) -> Option<Rc<Hosted>> {
    VIEWS.with(|v| v.borrow_mut().remove(&id))
}

pub fn get(id: WindowId) -> Option<Rc<Hosted>> {
    VIEWS.with(|v| v.borrow().get(&id).cloned())
}

fn all() -> Vec<Rc<Hosted>> {
    VIEWS.with(|v| v.borrow().values().cloned().collect())
}

/// Read what the shell sent since the last turn.
fn drain() {
    WAKE_QUEUED.store(false, Ordering::Release);
    loop {
        let Some(event) = RUNTIME.with(|r| r.get().and_then(|r| r.queue.try_recv().ok())) else {
            break;
        };
        match event {
            BlitzShellEvent::Poll { window_id } => {
                if let Some(h) = get(window_id) {
                    h.poll();
                }
            }
            BlitzShellEvent::RequestRedraw { doc_id } => {
                if let Some(h) = all().into_iter().find(|h| h.doc_id() == Some(doc_id)) {
                    schedule_redraw(h.id());
                }
            }
            BlitzShellEvent::ResumeReady { window_id } => {
                if let Some(h) = get(window_id) {
                    h.complete_resume();
                }
            }
            _ => {}
        }
    }
}

/// Draw `id` on the main queue's next turn, with whatever else asked by then.
pub fn schedule_redraw(id: WindowId) {
    REDRAWS.with(|r| r.borrow_mut().insert(id));
    if !REDRAW_QUEUED.swap(true, Ordering::AcqRel) {
        on_main(redraw);
    }
}

fn redraw() {
    REDRAW_QUEUED.store(false, Ordering::Release);
    let ids: Vec<WindowId> = REDRAWS.with(|r| r.borrow_mut().drain().collect());
    for id in ids {
        if let Some(h) = get(id)
            && !h.redraw()
        {
            // Busy — the view is mid-event. Its next turn, then.
            schedule_redraw(id);
        }
    }
}
