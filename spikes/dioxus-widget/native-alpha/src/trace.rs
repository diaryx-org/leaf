//! `LEAF_TRACE=1`: what each key, pointer event, IME event and repaint did,
//! on stderr. The spike's own instrument for the input path, which is where
//! Blitz and a browser differ most.

pub fn trace(line: impl FnOnce() -> String) {
    thread_local! {
        static ON: bool = std::env::var_os("LEAF_TRACE").is_some();
    }
    if ON.with(|on| *on) {
        eprintln!("{}", line());
    }
}
