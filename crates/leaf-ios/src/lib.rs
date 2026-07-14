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
    use gpui::{prelude::*, App, Focusable, WindowHandle, WindowOptions};
    use leaf_gpui::{register_keybindings, Editor, EditorCommand};

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

    // ── Editor window handle (so the native toolbar can target the editor) ──
    //
    // Main-thread-only, so an UnsafeCell behind a OnceLock is safe (same pattern
    // gpui-mobile uses for its app callback). WindowHandle is Copy.
    struct WindowCell(std::cell::UnsafeCell<Option<WindowHandle<Editor>>>);
    unsafe impl Send for WindowCell {}
    unsafe impl Sync for WindowCell {}
    static EDITOR_WINDOW: std::sync::OnceLock<WindowCell> = std::sync::OnceLock::new();

    fn store_editor_window(w: WindowHandle<Editor>) {
        let cell = EDITOR_WINDOW.get_or_init(|| WindowCell(std::cell::UnsafeCell::new(None)));
        unsafe { *cell.0.get() = Some(w) };
    }

    fn editor_window() -> Option<WindowHandle<Editor>> {
        EDITOR_WINDOW.get().and_then(|c| unsafe { *c.0.get() })
    }

    /// Run a formatting command on the editor, invoked from the native toolbar.
    ///
    /// Command ids are kept in sync with `ios/main.m`:
    ///   0 bold · 1 italic · 2 code · 3 H1 · 4 H2 · 5 body (¶)
    ///   · 6 toggle source/wysiwyg · 7 undo · 8 redo
    #[unsafe(no_mangle)]
    pub extern "C" fn leaf_ios_cmd(id: u32) {
        let cmd = match id {
            0 => EditorCommand::ToggleBold,
            1 => EditorCommand::ToggleItalic,
            2 => EditorCommand::ToggleCode,
            3 => EditorCommand::Heading1,
            4 => EditorCommand::Heading2,
            5 => EditorCommand::Paragraph,
            6 => EditorCommand::ToggleView,
            7 => EditorCommand::Undo,
            8 => EditorCommand::Redo,
            other => {
                log::warn!("leaf-ios: unknown toolbar command id {other}");
                return;
            }
        };
        let Some(win) = editor_window() else {
            log::warn!("leaf-ios: toolbar command with no editor window");
            return;
        };
        // Re-enter the gpui app — we're on the main thread but outside gpui's own
        // event dispatch (a UIKit button tap) — and run the command on the editor.
        let ran = gpui_mobile::ios::ffi::with_app(|cx| {
            win.update(cx, |editor, window, cx| {
                let before = editor.diag();
                editor.run_command(cmd, window, cx);
                let after = editor.diag();
                log::info!("leaf-ios: cmd {id} {cmd:?}\n  before: {before}\n  after:  {after}");
            })
        });
        match ran {
            None => log::warn!("leaf-ios: cmd {id} — app not running"),
            Some(Err(e)) => log::error!("leaf-ios: cmd {id} failed: {e:?}"),
            Some(Ok(())) => {}
        }
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
            Ok(window) => {
                log::info!("leaf-ios: editor window opened");
                // Remember the window so the native toolbar (`leaf_ios_cmd`) can
                // target this editor.
                store_editor_window(window);

                // Keep the caret above the software keyboard: feed the keyboard
                // height to the editor as a bottom inset whenever it changes.
                gpui_mobile::set_keyboard_height_callback(Box::new(|height| {
                    let Some(win) = editor_window() else { return };
                    gpui_mobile::ios::ffi::with_app(|cx| {
                        let _ = win.update(cx, |editor, _window, cx| {
                            editor.set_bottom_inset(gpui::px(height), cx);
                        });
                    });
                }));
                // Focus the editor so gpui registers its text-input handler for
                // the focused element — which is what brings up the soft keyboard
                // (see IosWindow::set_input_handler in the gpui-mobile fork).
                let _ = window.update(cx, |editor, window, cx| {
                    editor.focus_handle(cx).focus(window, cx);
                });
            }
            Err(e) => log::error!("leaf-ios: open_window failed: {e:?}"),
        }

        cx.activate(true);
    }

    /// Write a small markdown buffer to the app's temp dir and load it, so the
    /// editing surface has visible content. `Doc` loads from a path; iOS gives
    /// each app a writable temp directory (`std::env::temp_dir()`).
    fn seed_document() -> Option<leaf_core::Doc> {
        // Long enough to overflow the screen so touch-scroll is exercisable.
        const SAMPLE: &str = "\
# leaf on iOS 🍃

The **twig**-backed rich-text editor, now running on **gpui** via `gpui-mobile`
— Metal + wgpu + CoreText, on the iOS simulator.

## What this proves
- The same `Editor` widget as the desktop app, unchanged
- Native gpui rendering on iOS (not a web view)
- `source` and `wysiwyg` views share one render path

## Try it
- **Tap** the text to place the caret and raise the keyboard.
- **Type** — the buffer is live; markup resolves as you go.
- **Drag** to scroll; the keyboard dismisses on scroll.

## Markup
Headings, **bold**, *italic*, `code`, and lists all render inline, with the
delimiters hidden in the WYSIWYG view and revealed in source (⌘e / ⌥w).

1. First ordered item
2. Second ordered item
3. Third ordered item

> A blockquote, to show block-level styling on the phone.

### A deeper heading
Paragraphs keep flowing so there is enough content here to run past the bottom
of the screen and give the scroll gesture something to move. Keep dragging and
the text should glide under your finger, with momentum after you let go.

#### Even deeper
More text. More lines. The point is simply to overflow the viewport so the
`overflow_y_scroll` body actually has somewhere to go.

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
