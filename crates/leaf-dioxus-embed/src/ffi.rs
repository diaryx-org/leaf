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
use leaf_core::{Doc, Format, InlineKind, LineFlow, MarkColor, View};
use leaf_dioxus::{Look, TableCommand};

use crate::callbacks::Callbacks;
use crate::text;
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
    /// The caret's heading level, 0 in a paragraph.
    pub heading: u32,
    pub code_block: bool,
    pub verse: bool,
    pub blockquote: bool,
    /// The caret's task: -1 none, 0 open, 1 done.
    pub task: i32,
    /// The inline marks in force, `LEAF_MARK_*` bits.
    pub marks: u32,
    /// The caret is in a link (`leaf_view_link_at_caret` names it).
    pub link: bool,
    /// The highlight colour at the caret: -1 none, else `LEAF_COLOR_*`.
    pub mark_color: i32,
    /// Soft breaks drawn where they were written, rather than reflowed.
    pub preserve_breaks: bool,
}

/// `LEAF_MARK_*`, one bit per inline kind.
fn mark_bit(k: InlineKind) -> u32 {
    match k {
        InlineKind::Strong => 1,
        InlineKind::Emph => 2,
        InlineKind::Verbatim => 4,
        InlineKind::Mark => 8,
        InlineKind::Superscript => 16,
        InlineKind::Subscript => 32,
        InlineKind::Insert => 64,
        InlineKind::Delete => 128,
    }
}

/// `LEAF_COLOR_*`, in the order the menus list them.
const COLORS: [MarkColor; 7] = [
    MarkColor::Red,
    MarkColor::Orange,
    MarkColor::Yellow,
    MarkColor::Green,
    MarkColor::Blue,
    MarkColor::Purple,
    MarkColor::Brown,
];

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
                    heading: d.current_heading_level().unwrap_or(0),
                    code_block: d.caret_in_code_block(),
                    verse: d.caret_in_verse(),
                    blockquote: d.caret_in_blockquote(),
                    task: d.task_checked_at_caret().map_or(-1, i32::from),
                    marks: d.active_inline_marks().iter().map(mark_bit).sum(),
                    link: d.link_destination_at_caret().is_some(),
                    mark_color: d
                        .mark_color_at_caret()
                        .and_then(|c| COLORS.iter().position(|&k| k == c))
                        .map_or(-1, |i| i as i32),
                    preserve_breaks: d.line_flow() == LineFlow::Preserve,
                }
            })
        })
        .unwrap_or_default();
}

/// The destination of the link at the caret, or null. Free with
/// `leaf_string_free`.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_link_at_caret(h: *mut LeafView) -> *mut c_char {
    unsafe { hosted(h) }
        .and_then(|v| v.with_editor(|ed| ed.doc().link_destination_at_caret()))
        .flatten()
        .and_then(|s| CString::new(s).ok())
        .map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Make the selection a link to `destination`, or change the link at the
/// caret's destination — Format ▸ Link….
///
/// # Safety
/// `h` is null or a live handle; `destination` a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_insert_link(h: *mut LeafView, destination: *const c_char) {
    let (Some(v), Some(dest)) = (unsafe { hosted(h) }, unsafe { str_of(destination) }) else {
        return;
    };
    let dest = dest.to_owned();
    v.with_editor(|ed| {
        ed.doc().insert_link(&dest);
        ed.refresh();
    });
}

/// Highlight the selection, or recolour the highlight at the caret: a
/// `LEAF_COLOR_*`, or -1 for the plain highlighter.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_highlight(h: *mut LeafView, color: i32) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    let color = usize::try_from(color)
        .ok()
        .and_then(|i| COLORS.get(i).copied());
    v.with_editor(|ed| {
        ed.doc().highlight(color);
        ed.refresh();
    });
}

/// Draw soft breaks where they were written (`true`) or reflow them.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_set_preserve_breaks(h: *mut LeafView, preserve: bool) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    v.with_editor(|ed| {
        ed.doc().set_line_flow(if preserve {
            LineFlow::Preserve
        } else {
            LineFlow::Fold
        });
        ed.refresh();
    });
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

/// The selected text, or null with nothing selected. Free with
/// `leaf_string_free`.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_selected_text(h: *mut LeafView) -> *mut c_char {
    unsafe { hosted(h) }
        .and_then(|v| v.with_editor(|ed| ed.doc().selected_text().map(str::to_owned)))
        .flatten()
        .and_then(|s| CString::new(s).ok())
        .map_or(std::ptr::null_mut(), CString::into_raw)
}

/// The text the system indexes (see `text.rs`): the visible text, or with
/// `prose` the same with all but prose blanked, for the spelling checker.
/// Free with `leaf_string_free`.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_text(h: *mut LeafView, prose: bool) -> *mut c_char {
    let text = unsafe { hosted(h) }
        .and_then(|v| {
            v.with_editor(|ed| {
                let mut d = ed.doc();
                if prose {
                    text::prose(&mut d)
                } else {
                    text::visible(&mut d)
                }
            })
        })
        .unwrap_or_default();
    CString::new(text.replace('\0', " "))
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

/// The selection as a UTF-16 range of `leaf_view_text`: the caret alone is
/// an empty one.
///
/// # Safety
/// `h` is null or a live handle; `loc` and `len` point at `size_t`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_selected_range(
    h: *mut LeafView,
    loc: *mut usize,
    len: *mut usize,
) {
    let (lo, hi) = unsafe { hosted(h) }
        .and_then(|v| {
            v.with_editor(|ed| {
                let mut d = ed.doc();
                let (a, b) = d.selection().unwrap_or((d.caret, d.caret));
                (text::utf16_of(&mut d, a), text::utf16_of(&mut d, b))
            })
        })
        .unwrap_or_default();
    if let (Some(loc), Some(len)) = (unsafe { loc.as_mut() }, unsafe { len.as_mut() }) {
        *loc = lo;
        *len = hi.saturating_sub(lo);
    }
}

/// Select the UTF-16 range `loc`..`loc + len` of `leaf_view_text`; an empty
/// one places the caret.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_select_range(h: *mut LeafView, loc: usize, len: usize) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    v.with_editor(|ed| {
        {
            let mut d = ed.doc();
            let (from, to) = text::bounds(&mut d, loc, len);
            d.select_range(from, to);
            if len == 0 {
                d.anchor = None;
            }
        }
        ed.refresh();
    });
}

/// Replace the UTF-16 range `loc`..`loc + len` of `leaf_view_text` with
/// `text`, if the source bytes it shows spell `original` — false, and
/// nothing changed, if they do not. With `behind`, as an automatic
/// substitution behind the caret: the caret stays where typing left it and
/// the change is an undo step of its own. Otherwise as an edit, the caret
/// after it.
///
/// # Safety
/// `h` is null or a live handle; `original` and `text` C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_replace_range(
    h: *mut LeafView,
    loc: usize,
    len: usize,
    original: *const c_char,
    replacement: *const c_char,
    behind: bool,
) -> bool {
    let (Some(v), Some(original), Some(replacement)) =
        (unsafe { hosted(h) }, unsafe { str_of(original) }, unsafe {
            str_of(replacement)
        })
    else {
        return false;
    };
    v.with_editor(|ed| {
        let done = {
            let mut d = ed.doc();
            match text::exact(&mut d, loc, len, original) {
                Some((from, to)) if behind => d.substitute(from, to, replacement),
                Some((from, to)) => {
                    d.edit(from, to, replacement);
                    true
                }
                None => false,
            }
        };
        if done {
            ed.refresh();
        }
        done
    })
    .unwrap_or(false)
}

/// The UTF-16 index in `leaf_view_text` of the character at (`x`, `y`), in
/// points from the view's top-left; -1 off the text.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_index_at_point(h: *mut LeafView, x: f64, y: f64) -> i64 {
    unsafe { hosted(h) }
        .and_then(|v| {
            v.with_editor(|ed| {
                let off = ed.offset_at_point(x, y)?;
                Some(text::utf16_of(&mut ed.doc(), off) as i64)
            })
        })
        .flatten()
        .unwrap_or(-1)
}

/// Where the UTF-16 range `loc`..`loc + len` of `leaf_view_text` is drawn:
/// its first line's part of it, in points from the view's top-left, written
/// to `out` as x, y, width, height. False, and `out` untouched, where it is
/// not on a row of text.
///
/// # Safety
/// `h` is null or a live handle; `out` points at four doubles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_range_rect(
    h: *mut LeafView,
    loc: usize,
    len: usize,
    out: *mut f64,
) -> bool {
    let Some(rect) = unsafe { hosted(h) }
        .and_then(|v| {
            v.with_editor(|ed| {
                let (from, to) = text::bounds(&mut ed.doc(), loc, len);
                ed.rect_of(from, to)
            })
        })
        .flatten()
    else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    let out = unsafe { std::slice::from_raw_parts_mut(out, 4) };
    out.copy_from_slice(&[rect.0, rect.1, rect.2, rect.3]);
    true
}

/// Open (`begin`) or close an undo group: the edits between are one step.
///
/// # Safety
/// `h` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_undo_group(h: *mut LeafView, begin: bool) {
    if let Some(v) = unsafe { hosted(h) } {
        v.with_editor(|ed| {
            let mut d = ed.doc();
            if begin {
                d.begin_undo_group()
            } else {
                d.end_undo_group()
            }
        });
    }
}

/// The words the spelling checker found misspelled in the text at
/// `revision`: `count` UTF-16 ranges of `leaf_view_text`, as `ranges[2i]`
/// (location) and `ranges[2i + 1]` (length). Drawn underlined until the next
/// edit; ignored if the text has moved on since.
///
/// # Safety
/// `h` is null or a live handle; `ranges` is null or `2 * count` `size_t`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_view_set_misspellings(
    h: *mut LeafView,
    revision: u64,
    ranges: *const usize,
    count: usize,
) {
    let Some(v) = (unsafe { hosted(h) }) else {
        return;
    };
    let pairs = if ranges.is_null() || count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(ranges, count * 2) }
    };
    v.with_editor(|ed| {
        let found = {
            let mut d = ed.doc();
            if d.revision() != revision {
                return;
            }
            pairs
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[loc, len]| text::bounds(&mut d, loc, len))
                .filter(|(from, to)| from < to)
                .collect()
        };
        ed.set_misspellings(revision, found);
    });
}

/// The document `source` (UTF-8, `len` bytes) in `format`, at `path` for its
/// pictures (null for none), set in the body and code faces at `size` with
/// `leading`, on paper `paper_w` × `paper_h` points with a `margin`, as a
/// PDF: the bytes, `*out_len` long, to free with `leaf_bytes_free`; null if
/// it could not be made.
///
/// # Safety
/// `source` is `len` readable bytes; `path`, `body` and `mono` are null or C
/// strings; `out_len` points at a `size_t`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn leaf_pdf(
    source: *const u8,
    len: usize,
    format_code: u32,
    path: *const c_char,
    body: *const c_char,
    mono: *const c_char,
    size: f32,
    leading: f32,
    paper_w: f32,
    paper_h: f32,
    margin: f32,
    out_len: *mut usize,
) -> *mut u8 {
    let Some(out_len) = (unsafe { out_len.as_mut() }) else {
        return std::ptr::null_mut();
    };
    *out_len = 0;
    let bytes = if source.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(source, len) }
    };
    let text = String::from_utf8_lossy(bytes).into_owned();
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
    let path = unsafe { str_of(path) }.map(PathBuf::from);
    let paper = crate::pdf::Paper {
        width: paper_w,
        height: paper_h,
        margin,
    };
    crate::runtime::proxy();
    let pdf =
        crate::guard::guard(|| crate::pdf::render(&text, format(format_code), path, look, paper));
    match pdf {
        Some(Ok(pdf)) if !pdf.is_empty() => {
            let mut pdf = pdf.into_boxed_slice();
            *out_len = pdf.len();
            let ptr = pdf.as_mut_ptr();
            std::mem::forget(pdf);
            ptr
        }
        Some(Err(e)) => {
            eprintln!("leaf_pdf: {e}");
            std::ptr::null_mut()
        }
        _ => std::ptr::null_mut(),
    }
}

/// # Safety
/// `ptr` and `len` are a pair from `leaf_pdf`, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn leaf_bytes_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) });
    }
}
