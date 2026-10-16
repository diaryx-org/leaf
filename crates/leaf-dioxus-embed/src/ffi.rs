//! The C ABI: `include/leaf_dioxus_embed.h` declares every function here.
//!
//! A `LeafView` is a handle the host keeps for one editor in one of its views,
//! made by `leaf_view_new` and destroyed by `leaf_view_free` before the view
//! goes. Every function is called on the main thread. Strings cross as UTF-8:
//! the host's are borrowed for the call, and the editor's are the host's to
//! free with `leaf_string_free`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::str::FromStr;

use dioxus::html::{Code, Key};
use leaf_core::{Doc, Format, View};
use leaf_dioxus::{Look, TableCommand};

use crate::callbacks::Callbacks;
use crate::view::{Hosted, dioxus_mods};

/// The host's handle on one editor.
pub struct LeafView(Rc<Hosted>);

/// `LEAF_FORMAT_*`.
fn format(code: u32) -> Format {
    match code {
        1 => Format::Djot,
        2 => Format::Html,
        _ => Format::Markdown,
    }
}

/// A borrowed C string, or `None` for null or bad UTF-8.
///
/// # Safety
/// `s` is null or a NUL-terminated string live for the call.
unsafe fn str_of<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(s) }.to_str().ok()
}

/// # Safety
/// `h` is null or a live handle from `leaf_view_new`.
unsafe fn hosted<'a>(h: *mut LeafView) -> Option<&'a Hosted> {
    unsafe { h.as_ref() }.map(|v| &*v.0)
}

/// An editor in the host's view `view` (an `NSView *`), `width` × `height`
/// points at `scale`, on the document `source` (UTF-8, `len` bytes) in
/// `format`, at `path` (null for an untitled one). Null if the source would
/// not parse.
///
/// # Safety
/// `view` is a live view the host keeps until `leaf_view_free`; `source` is
/// `len` readable bytes; `path` is null or a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_new(
    view: *mut c_void,
    width: f64,
    height: f64,
    scale: f64,
    dark: bool,
    source: *const u8,
    len: usize,
    format_code: u32,
    path: *const c_char,
    callbacks: Callbacks,
) -> *mut LeafView {
    let Some(view) = NonNull::new(view) else {
        return std::ptr::null_mut();
    };
    let bytes = if source.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(source, len) }
    };
    let text = String::from_utf8_lossy(bytes).into_owned();
    let Ok(mut doc) = Doc::from_source(text, format(format_code)) else {
        return std::ptr::null_mut();
    };
    if let Some(p) = unsafe { str_of(path) } {
        doc.path = PathBuf::from(p);
    }
    let hosted = Hosted::new(view, (width, height, scale), dark, doc, callbacks);
    Box::into_raw(Box::new(LeafView(hosted)))
}

/// Destroy the editor. The host's view may go after this returns.
///
/// # Safety
/// `h` is null or a live handle, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_free(h: *mut LeafView) {
    if h.is_null() {
        return;
    }
    let v = unsafe { Box::from_raw(h) };
    crate::runtime::remove(v.0.id());
    drop(v);
}

/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_resize(h: *mut LeafView, width: f64, height: f64, scale: f64) {
    if let Some(v) = unsafe { hosted(h) } {
        v.resize(width, height, scale);
    }
}

/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_set_dark(h: *mut LeafView, dark: bool) {
    if let Some(v) = unsafe { hosted(h) } {
        v.set_dark(dark);
    }
}

/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_set_focused(h: *mut LeafView, focused: bool) {
    if let Some(v) = unsafe { hosted(h) } {
        v.set_focused(focused);
    }
}

/// The modifiers held (`LEAF_MOD_*`), sent before a pointer event whenever
/// they change.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_modifiers(h: *mut LeafView, mods: u32) {
    if let Some(v) = unsafe { hosted(h) } {
        v.modifiers(mods);
    }
}

/// The pointer, in points from the view's top-left corner. `kind` is 0 for a
/// move, 1 a press, 2 a release, 3 the pointer leaving; `button` 0 the main
/// button, 1 the secondary.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_pointer(
    h: *mut LeafView,
    kind: u32,
    x: f64,
    y: f64,
    button: u32,
    mods: u32,
) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    v.modifiers(mods);
    match kind {
        0 => v.pointer_moved(x, y),
        1 | 2 => {
            v.pointer_moved(x, y);
            v.pointer_button(x, y, button, kind == 1);
        }
        _ => v.pointer_left(),
    }
}

/// A scroll by (`dx`, `dy`): points when `precise` (a trackpad), else lines.
/// Positive `dy` moves toward the top, as AppKit's `scrollingDeltaY` does.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_scroll(h: *mut LeafView, dx: f64, dy: f64, precise: bool) {
    if let Some(v) = unsafe { hosted(h) } {
        v.scroll(dx, dy, precise);
    }
}

/// A key the host's view does not turn into text: a chord, an arrow, Return.
/// `key` is a DOM key value (`"b"`, `"ArrowUp"`, `"Enter"`), `code` a DOM
/// code (`"KeyB"`), `mods` the `LEAF_MOD_*` held. A menu item is its chord,
/// sent the same way.
///
/// # Safety
/// `h` is null or a live handle; `key` and `code` are C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_key(
    h: *mut LeafView,
    key: *const c_char,
    code: *const c_char,
    mods: u32,
) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    let Some(key) = (unsafe { str_of(key) }).and_then(|k| Key::from_str(k).ok()) else {
        return;
    };
    let code = unsafe { str_of(code) }
        .and_then(|c| Code::from_str(c).ok())
        .unwrap_or(Code::Unidentified);
    v.with_editor(|ed| ed.key(key, code, dioxus_mods(mods)));
}

/// Text from the host's text input client: typed, committed, picked, or
/// dictated.
///
/// # Safety
/// `h` is null or a live handle; `text` a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_insert_text(h: *mut LeafView, text: *const c_char) {
    if let (Some(v), Some(t)) = (unsafe { hosted(h) }, unsafe { str_of(text) }) {
        v.insert_text(t.to_owned());
    }
}

/// An input method's composition, `sel_start`..`sel_end` bytes of it
/// selected; an empty `text` withdraws it.
///
/// # Safety
/// `h` is null or a live handle; `text` a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_set_marked_text(
    h: *mut LeafView,
    text: *const c_char,
    sel_start: usize,
    sel_end: usize,
) {
    if let (Some(v), Some(t)) = (unsafe { hosted(h) }, unsafe { str_of(text) }) {
        let sel = (!t.is_empty()).then_some((sel_start.min(t.len()), sel_end.min(t.len())));
        v.set_marked(t.to_owned(), sel);
    }
}

/// Delete `before` bytes before the caret and `after` after it, ahead of an
/// insertion that replaces them.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_delete_surrounding(
    h: *mut LeafView,
    before: usize,
    after: usize,
) {
    if let Some(v) = unsafe { hosted(h) } {
        v.delete_surrounding(before, after);
    }
}

/// The document's source, for the host to save. Free with `leaf_string_free`.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_source(h: *mut LeafView) -> *mut c_char {
    let source = unsafe { hosted(h) }
        .and_then(|v| v.with_editor(|ed| ed.doc().source.clone()))
        .unwrap_or_default();
    CString::new(source.replace('\0', ""))
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// `s` is null or a string from this library, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Where the document now lives (after a Save As or a move), for its
/// pictures' relative paths; null for nowhere.
///
/// # Safety
/// `h` is null or a live handle; `path` null or a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_set_path(h: *mut LeafView, path: *const c_char) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    let path = unsafe { str_of(path) }
        .map(PathBuf::from)
        .unwrap_or_default();
    v.with_editor(|ed| {
        ed.doc().path = path;
        ed.refresh();
    });
}

/// The host saved the source it read: the document is clean as of now.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_mark_saved(h: *mut LeafView) {
    if let Some(v) = unsafe { hosted(h) } {
        v.with_editor(|ed| {
            ed.doc().dirty = false;
            ed.repaint();
        });
    }
}

/// What the host's menus and title read off the editor.
#[repr(C)]
#[derive(Default)]
pub struct LeafState {
    pub revision: u64,
    pub dirty: bool,
    pub source_view: bool,
    pub in_table: bool,
    pub has_selection: bool,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// # Safety
/// `h` is null or a live handle; `out` points at a `LeafState`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_state(h: *mut LeafView, out: *mut LeafState) {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    *out = unsafe { hosted(h) }
        .and_then(|v| {
            v.with_editor(|ed| {
                let mut d = ed.doc();
                LeafState {
                    revision: d.revision(),
                    dirty: d.dirty,
                    source_view: d.view == View::Source,
                    in_table: d.caret_in_table(),
                    has_selection: d.selection().is_some(),
                    can_undo: d.can_undo(),
                    can_redo: d.can_redo(),
                }
            })
        })
        .unwrap_or_default();
}

/// Draw with these choices: the body and code faces as CSS family lists, the
/// size in points, the leading, the measure in characters (0 or less fills
/// the view), and the appearance (0 the system's, 1 light, 2 dark).
///
/// # Safety
/// `h` is null or a live handle; `body` and `mono` are C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_set_look(
    h: *mut LeafView,
    body: *const c_char,
    mono: *const c_char,
    size: f32,
    leading: f32,
    measure: f32,
    appearance: u32,
) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    let mut look = Look::default();
    if let Some(b) = unsafe { str_of(body) } {
        look.ty.body = b.to_owned();
    }
    if let Some(m) = unsafe { str_of(mono) } {
        look.ty.mono = m.to_owned();
    }
    if size > 0.0 {
        look.ty.size = size;
    }
    if leading > 0.0 {
        look.ty.leading = leading;
    }
    look.measure = (measure > 0.0).then_some(measure);
    look.appearance = match appearance {
        1 => leaf_dioxus::Appearance::Light,
        2 => leaf_dioxus::Appearance::Dark,
        _ => leaf_dioxus::Appearance::System,
    };
    v.with_editor(|ed| ed.set_look(look));
}

/// A Table menu item: `LEAF_TABLE_*`, with `arg` its column count, side or
/// alignment.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_table(h: *mut LeafView, cmd: u32, arg: i32) {
    use leaf_core::Alignment;
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    let yes = arg != 0;
    let cmd = match cmd {
        0 => TableCommand::Insert {
            cols: arg.max(1) as usize,
        },
        1 => TableCommand::InsertRow { below: yes },
        2 => TableCommand::DeleteRow,
        3 => TableCommand::InsertColumn { right: yes },
        4 => TableCommand::DeleteColumn,
        5 => TableCommand::Align(match arg {
            1 => Alignment::Left,
            2 => Alignment::Center,
            3 => Alignment::Right,
            _ => Alignment::Default,
        }),
        6 => TableCommand::MoveRow { down: yes },
        7 => TableCommand::MoveColumn { right: yes },
        _ => return,
    };
    v.with_editor(|ed| ed.table(cmd));
}

/// An action no key stands for (`LEAF_ACTION_*`).
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_action(h: *mut LeafView, action: u32) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    v.with_editor(|ed| match action {
        1 => {
            ed.open_link();
        }
        2 => ed.copy_link(),
        3 => {
            ed.follow_footnote();
        }
        4 => edit(ed, Doc::insert_footnote),
        5 => edit(ed, Doc::insert_thematic_break),
        6 => edit(ed, Doc::insert_page_break),
        7 => edit(ed, Doc::toggle_verse),
        _ => {}
    });
}

fn edit(ed: &leaf_dioxus::Editor, f: fn(&mut Doc)) {
    f(&mut ed.doc());
    ed.refresh();
}
