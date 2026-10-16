//! Panics stop at the C ABI.
//!
//! Every call into the editor comes from the host's run loop through an
//! `extern "C"` function or a main-queue block, and a panic cannot unwind
//! through either: Rust aborts the process instead, and the host loses every
//! open document with it. So each way in runs its work under [`guard`], which
//! catches the panic, logs it, and lets the host carry on — the editor's
//! `RefCell`s are released as the panic unwinds, so the next event finds them
//! free. [`install_hook`] writes each panic's message and place to stderr and
//! to `~/Library/Logs/Leaf/panics.log`, where a crash report would not have it.

use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Once;

/// Run `f`, answering `None` if it panicked.
pub fn guard<R>(f: impl FnOnce() -> R) -> Option<R> {
    panic::catch_unwind(AssertUnwindSafe(f)).ok()
}

/// Log every panic, once per process.
pub fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let line = format!(
                "leaf-dioxus-embed panicked at {}: {}\n{}\n",
                info.location()
                    .map_or_else(|| "?".into(), |l| l.to_string()),
                message(info),
                std::backtrace::Backtrace::force_capture(),
            );
            if let Some(path) = log_path()
                && let Some(dir) = path.parent()
                && std::fs::create_dir_all(dir).is_ok()
                && let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
            {
                let _ = f.write_all(line.as_bytes());
            }
            default(info);
        }));
    });
}

fn message(info: &panic::PanicHookInfo<'_>) -> String {
    let p = info.payload();
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Logs/Leaf/panics.log"))
}
