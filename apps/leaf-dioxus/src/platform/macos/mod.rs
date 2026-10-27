//! The Mac: AppKit, through objc2.
//!
//! - [`document`]: `LeafDocument`, an `NSDocument`, so a file's life is the
//!   document system's — Open, Save, autosave in place, Versions, Rename,
//!   Move To, Revert, Open Recent, Finder's Open With.
//! - [`menubar`]: `menus::bar()` as the app's `NSMenu`s.
//! - [`spelling`]: the spelling checker, `NSSpellChecker`.
//! - [`lookup`]: the Dictionary's Look Up popover.
//! - [`print`]: Export as PDF's save panel and Print's print panel.
//!
//! And here: the application's delegate, where the menu bar's actions
//! arrive. winit registers none (its `winit-appkit` docs say so, and invite
//! one), so this one is the app's. It is the last stop on the action
//! responder chain before the document controller: an item with no target
//! that nothing in the key window answers — every editor command, and Cut,
//! Copy and Paste, which winit's view does not implement — lands here and
//! goes to the page in the main window. A save panel's text field comes
//! earlier in the chain, so ⌘V still pastes into it.

pub mod document;
pub mod lookup;
pub mod menubar;
pub mod print;
pub mod spelling;

use std::path::{Path, PathBuf};

use dioxus_native::winit::platform::macos::{EventLoopBuilderExtMacOS, WindowAttributesMacOS};
use dioxus_native::winit::window::{Window, WindowAttributes, WindowId};
use leaf_core::Format;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSDocumentController, NSFontManager, NSMenuItem,
    NSMenuItemValidation, NSView, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, NSURL};
use raw_window_handle::RawWindowHandle;

/// One document's handle, as the shared code holds it.
pub type DocumentRef = Retained<document::LeafDocument>;

use crate::app::{self, AppEvent};
use crate::commands::{ALT, CMD, Command, SHIFT};

/// The main thread's marker: everything here runs there.
pub fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("on the main thread")
}

/// The `NSView` winit draws a window into.
pub fn ns_view(window: &dyn Window) -> Option<Retained<NSView>> {
    let handle = window.rwh_06_window_handle().window_handle().ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return None;
    };
    // SAFETY: winit's handle names its live NSView.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    Some(view.retain())
}

/// The `NSWindow` a winit window is.
pub fn ns_window(window: &dyn Window) -> Option<Retained<NSWindow>> {
    ns_view(window)?.window()
}

/// Run `f` on the main queue's next turn, after winit's handler returns —
/// how anything that spins a nested event loop (a modal panel) is kept out
/// of the handler, which panics if it is handed an event while it runs.
pub fn later(f: impl FnOnce() + 'static) {
    struct OnMain(Box<dyn FnOnce()>);
    // SAFETY: made on the main thread and run on the main queue, which is
    // the main thread.
    unsafe impl Send for OnMain {}
    let job = OnMain(Box::new(f));
    dispatch2::DispatchQueue::main().exec_async(move || {
        let job = job;
        crate::guard::guard(job.0);
    });
}

/// The event loop's own settings: the menu bar is `menubar.rs`'s, not
/// winit's default one.
pub fn event_loop(builder: &mut dioxus_native::winit::event_loop::EventLoopBuilder) {
    builder.with_default_menu(false);
}

/// A document window's attributes: a transparent title bar over a page
/// that runs up behind it, so the toolbar's band is the title bar's, as a
/// unified Mac toolbar is; and tabs, as every document app has them.
pub fn document_window(attributes: WindowAttributes) -> WindowAttributes {
    attributes.with_platform_attributes(Box::new(
        WindowAttributesMacOS::default()
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_tabbing_identifier("org.diaryx.leaf.document"),
    ))
}

/// How far a document window's page runs up behind the title bar.
pub const TITLE_BAR: f64 = 28.0;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LeafAppDelegate"]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        /// A Mac document app outlives its last window.
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window(&self, _app: &NSApplication) -> bool {
            false
        }
    }

    impl AppDelegate {
        /// A menu bar item that is one of the app's commands.
        #[unsafe(method(leafCommand:))]
        fn leaf_command(&self, sender: &NSMenuItem) {
            if let Some(c) = menubar::command(sender.tag()) {
                run(c);
            }
        }

        // The Edit menu's selectors: a system text field answers them
        // first; in a document window they are the editor's chords.
        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) {
            run(edit_chord(sel!(cut:)));
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            run(edit_chord(sel!(copy:)));
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            run(edit_chord(sel!(paste:)));
        }

        #[unsafe(method(pasteAsPlainText:))]
        fn paste_plain(&self, _sender: Option<&AnyObject>) {
            run(edit_chord(sel!(pasteAsPlainText:)));
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            run(edit_chord(sel!(selectAll:)));
        }
    }

    unsafe impl NSMenuItemValidation for AppDelegate {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            validate(item)
        }
    }
);

/// Whether a menu item can be chosen now, ticking it where what it does is
/// in force.
fn validate(item: &NSMenuItem) -> bool {
    let Some(action) = item.action() else {
        return true;
    };
    let command = if action == sel!(leafCommand:) {
        menubar::command(item.tag())
    } else {
        Some(edit_chord(action))
    };
    let Some(c) = command else { return false };
    let page = main_page().and_then(app::page);
    let chrome = page.as_ref().and_then(|p| p.chrome());
    let s = crate::commands::status(&c, chrome.as_ref(), &app::app_state());
    item.setState(if s.checked {
        objc2_app_kit::NSControlStateValueOn
    } else {
        objc2_app_kit::NSControlStateValueOff
    });
    s.enabled
}

/// The Edit menu's selectors as the editor's chords.
fn edit_chord(action: objc2::runtime::Sel) -> Command {
    use dioxus::html::Code;
    if action == sel!(cut:) {
        Command::chord("x", Code::KeyX, CMD)
    } else if action == sel!(copy:) {
        Command::chord("c", Code::KeyC, CMD)
    } else if action == sel!(paste:) {
        Command::chord("v", Code::KeyV, CMD)
    } else if action == sel!(pasteAsPlainText:) {
        Command::chord("v", Code::KeyV, CMD | SHIFT | ALT)
    } else {
        Command::chord("a", Code::KeyA, CMD)
    }
}

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// The page in the main window — the document window in front, or the
/// Settings window — or, while the app is not active and AppKit names no
/// main window, the window that last had focus.
pub fn main_page() -> Option<WindowId> {
    let main = NSApplication::sharedApplication(mtm()).mainWindow();
    let pages = app::pages();
    main.and_then(|main| {
        pages
            .iter()
            .find(|p| ns_window(&*p.window).is_some_and(|w| *w == *main))
            .map(|p| p.window.id())
    })
    .or_else(|| app::last_focused().filter(|id| app::is_open(*id)))
}

/// Carry out a menu bar command: the app's own, or the main window's.
pub fn run(c: Command) {
    if c.is_app() {
        app::run_app(c);
    } else if let Some(id) = main_page() {
        app::post(AppEvent::Command(id, c));
    }
}

thread_local! {
    static DELEGATE: std::cell::OnceCell<Retained<AppDelegate>> = const { std::cell::OnceCell::new() };
}

/// Before the loop runs: the document class registered (AppKit looks it up
/// by the name `Info.plist` gives it), the delegate set, the menu bar
/// installed.
pub fn start() {
    let mtm = mtm();
    document::register();
    let app = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|d| d.set(delegate).ok());
    menubar::install(mtm);
    spelling::start();
}

/// The loop is running: open what the command line names. A file handed
/// over by Finder, `open` or the Dock arrives through AppKit instead, which
/// gives it to the document controller itself.
pub fn launched() {
    if std::env::args().any(|a| a == "--sample") {
        open_sample();
    }
    for file in std::env::args().skip(1).filter(|a| !a.starts_with('-')) {
        open_file(Path::new(&file));
    }
}

/// Open `path` as Finder would: a window of its own, or the one it already
/// has brought forward.
pub fn open_file(path: &Path) {
    let Ok(path) = path.canonicalize() else {
        eprintln!("{}: no such file", path.display());
        return;
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let controller = NSDocumentController::sharedDocumentController(mtm());
    let done = block2::RcBlock::new(
        |_doc: *mut objc2_app_kit::NSDocument,
         _shown: objc2::runtime::Bool,
         error: *mut objc2_foundation::NSError| {
            if let Some(e) = unsafe { error.as_ref() } {
                eprintln!("{}", e.localizedDescription());
            }
        },
    );
    controller.openDocumentWithContentsOfURL_display_completionHandler(&url, true, &done);
}

/// File ▸ New: an untitled document of `format`, through the document
/// controller, so that it is the document system's — its window, its Save
/// As, its autosave — exactly as AppKit's own New would make one.
pub fn new_document(format: Format) {
    let controller = NSDocumentController::sharedDocumentController(mtm());
    let ty = NSString::from_str(document::type_of(format));
    match controller.makeUntitledDocumentOfType_error(&ty) {
        Ok(doc) => {
            controller.addDocument(&doc);
            doc.makeWindowControllers();
            doc.showWindows();
        }
        Err(e) => eprintln!("new document: {}", e.localizedDescription()),
    }
}

/// File ▸ Open…: the document controller's Open panel. The menu bar's Open
/// is the system's own item; this is for the commands that name it.
pub fn open_panel() {
    let controller = NSDocumentController::sharedDocumentController(mtm());
    let _: () = unsafe { msg_send![&*controller, openDocument: None::<&AnyObject>] };
}

/// Quit, as the app menu's Quit does: each document asked, then the app.
pub fn quit() {
    NSApplication::sharedApplication(mtm()).terminate(None);
}

/// A Mac document app outlives its last window.
pub const QUITS_WITH_LAST_WINDOW: bool = false;

/// The menu bar has every command's key, before the window sees it.
pub fn shortcut(
    _page: &crate::page::Page,
    _code: &dioxus::html::Code,
    _mods: dioxus::html::Modifiers,
) -> bool {
    false
}

/// Help ▸ Open Sample Document: an untitled Markdown document holding it.
pub fn open_sample() {
    document::sample_next();
    new_document(Format::Markdown);
}

/// Where the bundle keeps its resources — the sample and its pictures, side
/// by side. Run from `cargo run`, outside a bundle, the app's own
/// `resources/Media`, where the pictures come from.
pub fn resources() -> PathBuf {
    let bundled = objc2_foundation::NSBundle::mainBundle()
        .resourcePath()
        .map(|p| PathBuf::from(p.to_string()));
    match bundled {
        Some(dir) if dir.join("Sample.md").exists() => dir,
        _ => PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/Media")),
    }
}

/// The bundled sample document's text.
pub fn sample() -> String {
    let dev = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/Sample.md");
    std::fs::read_to_string(resources().join("Sample.md"))
        .or_else(|_| std::fs::read_to_string(dev))
        .unwrap_or_else(|_| "# leaf\n\nThe sample document is missing from the bundle.\n".into())
}

/// Hand a URL, or a file, to the app the system opens it with.
pub fn open_url(what: &str) {
    let url = if what.contains("://") || what.starts_with("mailto:") {
        NSURL::URLWithString(&NSString::from_str(what))
    } else {
        Some(NSURL::fileURLWithPath(&NSString::from_str(what)))
    };
    if let Some(url) = url {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// The emoji and symbols picker. Its choice arrives as an `Ime::Commit`
/// (see the Blitz fork's `patches/winit-appkit/PATCH.md`).
pub fn character_palette() {
    NSApplication::sharedApplication(mtm()).orderFrontCharacterPalette(None);
}

/// Every font family installed, hidden ones (a leading `.`) aside.
pub fn font_families() -> Vec<String> {
    NSFontManager::sharedFontManager(mtm())
        .availableFontFamilies()
        .iter()
        .map(|f| f.to_string())
        .filter(|f| !f.starts_with('.'))
        .collect()
}

/// The app's user defaults, by string.
pub mod defaults {
    use objc2_foundation::{NSString, NSUserDefaults};

    /// The value under `key`, as a string: a Bool, a number, reads as "0"
    /// or "1".
    pub fn string(key: &str) -> Option<String> {
        NSUserDefaults::standardUserDefaults()
            .stringForKey(&NSString::from_str(key))
            .map(|s| s.to_string())
    }

    pub fn set(key: &str, value: &str) {
        unsafe {
            NSUserDefaults::standardUserDefaults()
                .setObject_forKey(Some(&NSString::from_str(value)), &NSString::from_str(key))
        };
    }

    /// Store a Bool as `@AppStorage` does.
    pub fn set_bool(key: &str, value: bool) {
        NSUserDefaults::standardUserDefaults().setBool_forKey(value, &NSString::from_str(key));
    }

    pub fn remove(key: &str) {
        NSUserDefaults::standardUserDefaults().removeObjectForKey(&NSString::from_str(key));
    }
}

/// The general pasteboard, with the HTML flavour beside the text, as the
/// editor publishes both.
pub mod clipboard {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    pub fn write(text: &str, html: Option<&str>) -> Result<(), String> {
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        let ok =
            unsafe { board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
        if let Some(html) = html {
            unsafe { board.setString_forType(&NSString::from_str(html), NSPasteboardTypeHTML) };
        }
        if ok {
            Ok(())
        } else {
            Err("the pasteboard refused the text".into())
        }
    }

    pub fn read() -> leaf_dioxus::Clipboard {
        let board = NSPasteboard::generalPasteboard();
        let get = |ty| board.stringForType(ty).map(|s| s.to_string());
        leaf_dioxus::Clipboard {
            text: get(unsafe { NSPasteboardTypeString }),
            html: get(unsafe { NSPasteboardTypeHTML }),
        }
    }
}
