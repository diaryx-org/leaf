//! leaf-ios — iOS host for the embeddable [`leaf_gpui::Editor`], running on the
//! gpui-mobile platform layer (Metal via wgpu, CoreText text shaping).
//!
//! This crate is deliberately thin: it is the iOS analogue of `crates/leaf`'s
//! `main.rs`, with gpui-mobile's UIKit run-loop swapped in for the desktop
//! `gpui_platform::application()` runner. The `Editor` widget itself is shared
//! verbatim — it touches only `gpui`, never a platform backend.
//!
//! ## Entry contract (see the Xcode harness `ios/main.m`)
//!
//! `application:didFinishLaunchingWithOptions:` calls two C symbols:
//!   1. [`gpui_ios_register_app`] — defined HERE; registers the root-view builder.
//!   2. `gpui_ios_run_demo()` — defined in gpui-mobile; sets up the platform and
//!      enters the run loop, invoking our registered callback.

// Force-link gpui-mobile so its `gpui_ios_*` platform FFI symbols (run loop,
// lifecycle, frame requests, touch) are pulled into libleaf_ios.a even though
// nothing in Rust references them directly.
extern crate gpui_mobile;

#[cfg(target_os = "ios")]
mod ios {
    // `prelude::*` brings gpui's context traits into scope so `cx.new(...)`
    // (entity construction) resolves on `&mut App`.
    use gpui::{prelude::*, App, WindowOptions};
    use leaf_gpui::{register_keybindings, Editor};

    /// Register the leaf root view with the gpui-mobile iOS platform.
    ///
    /// Must be called (from `main.m`) BEFORE `gpui_ios_run_demo()` so the run
    /// loop knows which view to create.
    #[unsafe(no_mangle)]
    pub extern "C" fn gpui_ios_register_app() {
        init_logging();
        log::info!("leaf-ios: registering root view");
        gpui_mobile::ios::ffi::set_app_callback(Box::new(|cx: &mut App| {
            open_editor_window(cx);
        }));
    }

    /// Open a fullscreen window whose root view is the leaf `Editor`.
    fn open_editor_window(cx: &mut App) {
        // The editor's own keybindings (⌘e view toggle, ⌘b/⌘i, …). Harmless
        // without a hardware keyboard; the soft-keyboard bridge is future work.
        register_keybindings(cx);

        // Seed a small welcome document so the editing surface opens with real
        // content. (Relies on gpui-mobile's iOS CoreText font-trait fix — without
        // it, shaping any text aborts on the simulator.)
        match cx.open_window(
            WindowOptions {
                window_bounds: None,
                ..Default::default()
            },
            move |_, cx| cx.new(|cx| Editor::new(cx, seed_document())),
        ) {
            Ok(_) => log::info!("leaf-ios: editor window opened"),
            Err(e) => log::error!("leaf-ios: open_window failed: {e:?}"),
        }

        cx.activate(true);
    }

    /// Write a small markdown buffer to the app's temp dir and load it, so the
    /// editing surface has visible content. `Doc` loads from a path; iOS gives
    /// each app a writable temp directory (`std::env::temp_dir()`).
    fn seed_document() -> Option<leaf_core::Doc> {
        const SAMPLE: &str = "\
# leaf on iOS 🍃

The **twig**-backed rich-text editor, now running on **gpui** via `gpui-mobile`
— Metal + wgpu + CoreText, on the iOS simulator.

## What this proves
- The same `Editor` widget as the desktop app, unchanged
- Native gpui rendering on iOS (not a web view)
- `source` and `wysiwyg` views share one render path

*This text is the leaf editor's own buffer — you are looking at `Editor`.*
";
        let mut path = std::env::temp_dir();
        path.push("welcome.md");
        if let Err(e) = std::fs::write(&path, SAMPLE) {
            log::error!("leaf-ios: could not write sample doc: {e:?}");
            return None;
        }
        match leaf_core::Doc::open(path) {
            Ok(doc) => Some(doc),
            Err(e) => {
                log::error!("leaf-ios: Doc::open failed: {e:?}");
                None
            }
        }
    }

    // ── logging: route Rust `log` + panics through NSLog so they appear in the
    //    simulator/device console. Adapted from gpui-mobile's example. ──────────

    struct NsLogLogger;

    impl log::Log for NsLogLogger {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            nslog(&format!(
                "[{}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            ));
        }
        fn flush(&self) {}
    }

    fn init_logging() {
        let _ = log::set_logger(&NsLogLogger)
            .map(|()| log::set_max_level(log::LevelFilter::Info));
        std::panic::set_hook(Box::new(|info| nslog(&format!("leaf-ios PANIC: {info}"))));
    }

    fn nslog(msg: &str) {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        unsafe {
            extern "C" {
                fn NSLog(fmt: *mut AnyObject, ...);
            }
            let c_msg = std::ffi::CString::new(msg).unwrap_or_default();
            let ns_msg: *mut AnyObject = msg_send![class!(NSString), alloc];
            let ns_msg: *mut AnyObject =
                msg_send![ns_msg, initWithUTF8String: c_msg.as_ptr()];
            let c_fmt = std::ffi::CString::new("%@").unwrap_or_default();
            let ns_fmt: *mut AnyObject = msg_send![class!(NSString), alloc];
            let ns_fmt: *mut AnyObject =
                msg_send![ns_fmt, initWithUTF8String: c_fmt.as_ptr()];
            NSLog(ns_fmt, ns_msg);
        }
    }
}
