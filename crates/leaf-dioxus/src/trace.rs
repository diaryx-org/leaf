//! `LEAF_TRACE=1`: what each key, pointer event, IME event and repaint did,
//! on stderr, each line stamped with the milliseconds since the first. The
//! editor's own instrument for the input path, which is where Blitz and a
//! browser differ most.

use std::sync::OnceLock;
use std::time::Instant;

pub fn trace(line: impl FnOnce() -> String) {
    thread_local! {
        static ON: bool = std::env::var_os("LEAF_TRACE").is_some();
    }
    static START: OnceLock<Instant> = OnceLock::new();
    if ON.with(|on| *on) {
        let at = START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0;
        eprintln!("{at:9.1} {}", line());
    }
}
