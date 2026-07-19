//! leaf-gpui — an **embeddable** caret-based rich-text editor widget for gpui,
//! built on twig via `leaf-core`.
//!
//! The public surface is the [`Editor`] entity: a host drops it into its own
//! gpui view, calls [`register_keybindings`] once, and themes it with an
//! [`EditorStyle`]. It renders only the editing surface — window chrome, file
//! I/O, and quit stay with the host (the `leaf` binary is one such host).
//!
//! Sibling to `leaf-tui`: **same core, different surface.** Every hard part —
//! the byte-offset caret, the selection model, and turning each edit into one of
//! twig's offset-addressed ops — lives in `leaf-core` and is shared verbatim.
//! This crate only paints glyphs with gpui's text system and forwards keyboard /
//! mouse events into the same `Doc` methods the terminal frontend calls.
//!
//! Two views, toggled with `⌘e`, exactly as the TUI toggles with `⌥w`:
//!   - **source** — the document's raw text, caret in source bytes.
//!   - **wysiwyg** — `leaf-core`'s `VisualMap` resolved: `**bold**` painted bold,
//!     `# ` / `**` / `` ` `` delimiters hidden, headings coloured. Each rendered
//!     glyph still points at its source byte, so the caret rides the *visible*
//!     text and steps over hidden delimiters — the same map the TUI renders.
//!
//! Both views share one rendering path: a list of [`RowLayout`]s, each a shaped
//! line plus, per character, the source offset it maps back to. Caret placement,
//! selection rectangles, and mouse hit-testing all read that, so they work
//! identically whether the row came from a source line or a `VisualMap` row.

mod prompt;
mod style;

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::sync::Arc;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Context, Corners, CursorStyle, DevicePixels, Element,
    ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyBinding, KeyDownEvent, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render,
    RenderImage, ScrollHandle, SharedString, ShapedLine, Size, Style, Task, TextAlign,
    UTF16Selection, UnderlineStyle, Window, actions, anchored, deferred, div, fill, point,
    prelude::*, px, quad, relative, rgb, rgba, size,
};
use leaf_core::style::{Role, Style as CoreStyle};
use prompt::{PromptAction, TextPrompt};

/// How long the caret rests in each blink phase.
///
/// A constant, not a platform query: the pinned gpui surfaces no caret-blink
/// preference at all — neither an interval nor the accessibility "blink off"
/// switch (grepping `crates/gpui/src` for `blink` finds nothing outside its own
/// examples), so there is nothing to respect here even though macOS itself has
/// `NSTextInsertionPointBlinkPeriod`. 500ms is what gpui's own editor example
/// and Zed's `BlinkManager` both hard-code, and it matches the AppKit default.
const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// A formatting / view / history command that can be run programmatically —
/// e.g. from a native iOS toolbar — equivalent to the corresponding keybound
/// action. Dispatched via [`Editor::run_command`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    ToggleBold,
    ToggleItalic,
    ToggleCode,
    ToggleMark,
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    ToggleView,
    Undo,
    Redo,
    Save,
}

/// The widget's visual theme — the handful of colors and text metrics a host can
/// override to match its own look. `Default` reproduces the standalone app's
/// appearance (light background, blue caret). Pass one with [`Editor::set_style`].
#[derive(Clone)]
pub struct EditorStyle {
    /// The editing surface's background.
    pub background: Hsla,
    /// Default (unstyled) glyph color; markup styling layers over it.
    pub text: Hsla,
    /// The caret bar.
    pub caret: Hsla,
    /// The selection highlight (typically translucent).
    pub selection: Hsla,
    /// A table's rules, the fill behind its header, and the tint on every other
    /// body row. The TUI spells a table with box glyphs; here the borders are
    /// real geometry, so they need real colors.
    pub table_border: Hsla,
    pub table_header: Hsla,
    pub table_stripe: Hsla,
    /// A code block's border and the tint behind it — the box a fenced or
    /// indented block is set apart by now that core draws no gutter. The same
    /// `code_background` also fills the pill behind an inline `` `code` `` run.
    pub code_border: Hsla,
    pub code_background: Hsla,
    /// The GUI's palette for leaf-core's semantic roles (see [`crate::style`]).
    /// Core no longer bakes colors in — a role is all it records — so a heading,
    /// code, and body text all read in [`Self::text`]; only these three roles
    /// take a color of their own. `link` is a hyperlink; `muted` is quiet
    /// decoration (bullets, quote/code gutters, rules); `mark_background` is the
    /// highlight behind `==marked==` text.
    pub link: Hsla,
    pub muted: Hsla,
    pub mark_background: Hsla,
    /// Body font family.
    pub font_family: SharedString,
    /// The monospace family code (inline `` `verbatim` `` and fenced blocks) is
    /// drawn in — the GUI's answer to leaf-core's `Role::Code`, where the TUI,
    /// already monospace, needs nothing. Kept separate from `font_family` so the
    /// body can be proportional while code still lines up in columns.
    pub mono_font_family: SharedString,
    /// Body font size and line height. A heading's size is this scaled by
    /// [`Self::heading_scale`]; its row is proportionally taller (the line
    /// height tracks the font size), which is why the widget lays rows out at
    /// their own heights rather than on one uniform grid.
    pub font_size: Pixels,
    pub line_height: Pixels,
    /// The height of a between-blocks gap row, as a fraction of `line_height`.
    /// Core spells a block boundary with an empty decoration row; drawn at a full
    /// line box it reads as a blank line the user never typed, so it's laid out
    /// short — ordinary paragraph spacing. `1.0` restores the old full-line gap.
    pub block_gap_scale: f32,
    /// How much larger than the body each heading level is drawn, `[h1, …, h6]`.
    /// Headings are distinguished by size and weight alone (no color), so this
    /// ramp is the whole hierarchy. The default is a moderate one — h1 ≈ 1.6×
    /// body, tapering to body size by h5.
    pub heading_scale: [f32; 6],
}

impl Default for EditorStyle {
    fn default() -> Self {
        EditorStyle {
            background: gpui::white(),
            text: rgb(0x1e1e1e).into(),
            caret: gpui::blue(),
            selection: rgba(0x3311ff30).into(),
            table_border: rgb(0xd0d0d0).into(),
            table_header: rgb(0xf0f0f0).into(),
            table_stripe: rgb(0xf8f8f8).into(),
            code_border: rgb(0xe0e0e0).into(),
            code_background: rgb(0xf5f5f5).into(),
            link: rgb(0x1e66f5).into(),
            muted: rgb(0x9a9a9a).into(),
            mark_background: rgb(0xfaf0a0).into(),
            font_family: "Helvetica".into(),
            mono_font_family: "Menlo".into(),
            font_size: px(16.0),
            line_height: px(24.0),
            block_gap_scale: 0.5,
            // 26 / 22 / 19 / 17 / 16 / 15 px against a 16px body.
            heading_scale: [1.625, 1.375, 1.1875, 1.0625, 1.0, 0.9375],
        }
    }
}
use leaf_core::{
    Alignment, BlockKind, ColorScheme, DiskState, Doc, Glyph, ImageInfo, InlineKind, InlineMarks,
    TableInfo, View,
};

use crate::style::{RunStyle, heading_scale, text_run};

/// Something the widget needs its host to do, because only the host can: the
/// editor owns the document, but the window and the process are the app's.
/// A host wires these with `cx.subscribe` (see the `leaf` binary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// The close question has been answered, and the answer was "go": the
    /// document is saved, or its loss was deliberately chosen. A host that
    /// asked with [`Editor::confirm_close`] and got `false` quits when this
    /// arrives.
    CloseConfirmed,
}

impl EventEmitter<EditorEvent> for Editor {}

/// What to do once a dirty document has been dealt with — the reason a dialog
/// or a Save As prompt is up at all, carried through however many steps that
/// takes. "Save and quit" on an untitled document is the long way round: a
/// dialog, then a prompt for the name, then a write, and only then the quit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// A plain ⌘S / ⌘⇧S: the save is the whole errand.
    Nothing,
    /// Quit once the bytes are down (or deliberately abandoned).
    Quit,
    /// Replace the document with a blank one — ⌘N over unsaved work.
    NewDoc,
}

/// A question the editor is putting to the user, and the only thing on screen
/// that can answer it: while one is up, `render` gates every document key and
/// mouse listener off exactly as it does for the modal prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialog {
    /// Unsaved work is about to be dropped on the floor — by a quit, or by ⌘N.
    /// Save / Discard / Cancel, which is the whole point: the old ⌘Q armed a
    /// bit and the *second* press discarded the document, with the header left
    /// to suggest the user go press ⌘S themselves.
    Discard { then: Pending },
    /// The file changed under a document with unsaved edits and a save is what
    /// asked. Saving overwrites their work, reloading discards ours; leaf-core
    /// deliberately won't choose (see `Doc::disk_state`), so neither will this.
    Overwrite { then: Pending },
    /// The file changed under us, noticed on window activation rather than on
    /// the way to anything.
    DiskChanged,
}

actions!(
    leaf,
    [
        Backspace, Delete, Left, Right, Up, Down, SelectLeft, SelectRight, SelectUp, SelectDown,
        Home, End, SelectHome, SelectEnd, Newline, Indent, Save, ToggleBold, ToggleItalic,
        ToggleView,
        // Word motion / deletion (⌥←/→, ⌥⌫/⌦) and select-all (⌘A) — the same
        // leaf-core word-boundary ops the TUI already binds.
        MoveWordLeft, MoveWordRight, SelectWordLeft, SelectWordRight, DeleteWordBack,
        DeleteWordForward, SelectAll,
        // Format parity with the TUI's ⌥ toolbar (⌥c/⌥m/⌥0-6): code, mark, and
        // block kind. The GUI keeps ⌥ for word motion, so these ride ⌘⇧ / ⌃.
        ToggleCode, ToggleMark, Paragraph, Heading1, Heading2, Heading3, Heading4, Heading5,
        Heading6,
        // Clipboard (⌘C/⌘X/⌘V), plus ⌘⇧V for the plain flavor. Backed by arboard
        // on the desktop, not by gpui's clipboard — see `set_clipboard`.
        Copy, Cut, Paste, PasteAsPlainText,
        // History (⌘Z / ⇧⌘Z).
        Undo, Redo,
        // Document start/end (⌘↑ / ⌘↓) and page motion, with ⇧ selecting.
        DocStart, DocEnd, SelectDocStart, SelectDocEnd,
        PageUp, PageDown, SelectPageUp, SelectPageDown,
        // Blockquote / list containers (⌘⇧9/8/7) and the link prompt (⌘K) —
        // the toolbar's remaining format commands, mirroring the TUI's set.
        ToggleBlockquote, ToggleBulletList, ToggleOrderedList, InsertLink,
        // Set the language of the fenced code block at the caret (⌘⇧L).
        SetLanguage,
        // Indentation (⇥/⇧⇥) and the line kills (⌘⌫/⌃K).
        Outdent, DeleteToLineStart, DeleteToLineEnd,
        // Strikethrough / underline — twig's Delete / Insert inline kinds.
        ToggleStrikethrough, ToggleUnderline,
        // Document lifecycle the widget owns: Save As (⌘⇧S) and a new blank
        // document (⌘N). Plain quit/open stay the host's.
        SaveAs, NewDocument,
    ]
);

/// Bind the editor's keys to their actions, scoped to the `Editor` key context
/// so they only fire when an embedded editor is focused and never clash with a
/// host application's own bindings. A host calls this once at startup; the
/// standalone app does too. App-level keys (quit, open) are the host's to bind.
pub fn register_keybindings(cx: &mut App) {
    let ctx = Some("Editor");
    cx.bind_keys([
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("shift-home", SelectHome, ctx),
        KeyBinding::new("shift-end", SelectEnd, ctx),
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("enter", Newline, ctx),
        KeyBinding::new("tab", Indent, ctx),
        KeyBinding::new("cmd-b", ToggleBold, ctx),
        KeyBinding::new("cmd-i", ToggleItalic, ctx),
        KeyBinding::new("cmd-e", ToggleView, ctx),
        KeyBinding::new("cmd-s", Save, ctx),
        KeyBinding::new("cmd-z", Undo, ctx),
        KeyBinding::new("cmd-shift-z", Redo, ctx),
        // ⌘Y: the Windows/CUA redo convention, same alias the TUI accepts (^Y).
        KeyBinding::new("cmd-y", Redo, ctx),
        KeyBinding::new("alt-left", MoveWordLeft, ctx),
        KeyBinding::new("alt-right", MoveWordRight, ctx),
        KeyBinding::new("shift-alt-left", SelectWordLeft, ctx),
        KeyBinding::new("shift-alt-right", SelectWordRight, ctx),
        KeyBinding::new("alt-backspace", DeleteWordBack, ctx),
        KeyBinding::new("alt-delete", DeleteWordForward, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-shift-c", ToggleCode, ctx),
        KeyBinding::new("cmd-shift-m", ToggleMark, ctx),
        KeyBinding::new("ctrl-0", Paragraph, ctx),
        KeyBinding::new("ctrl-1", Heading1, ctx),
        KeyBinding::new("ctrl-2", Heading2, ctx),
        KeyBinding::new("ctrl-3", Heading3, ctx),
        KeyBinding::new("ctrl-4", Heading4, ctx),
        KeyBinding::new("ctrl-5", Heading5, ctx),
        KeyBinding::new("ctrl-6", Heading6, ctx),
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-x", Cut, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-shift-v", PasteAsPlainText, ctx),
        KeyBinding::new("cmd-up", DocStart, ctx),
        KeyBinding::new("cmd-down", DocEnd, ctx),
        KeyBinding::new("cmd-shift-up", SelectDocStart, ctx),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, ctx),
        KeyBinding::new("pageup", PageUp, ctx),
        KeyBinding::new("pagedown", PageDown, ctx),
        KeyBinding::new("shift-pageup", SelectPageUp, ctx),
        KeyBinding::new("shift-pagedown", SelectPageDown, ctx),
        // Blockquote / list / link — ⌘⇧ rather than ⌥ for the same reason as
        // the code/mark toggles above (⌥ stays reserved for word motion).
        KeyBinding::new("cmd-shift-9", ToggleBlockquote, ctx),
        KeyBinding::new("cmd-shift-8", ToggleBulletList, ctx),
        KeyBinding::new("cmd-shift-7", ToggleOrderedList, ctx),
        KeyBinding::new("cmd-k", InsertLink, ctx),
        KeyBinding::new("cmd-shift-l", SetLanguage, ctx),
        KeyBinding::new("shift-tab", Outdent, ctx),
        // The line kills, as `NSStandardKeyBindingResponding` spells them:
        // ⌘⌫ is `deleteToBeginningOfLine:` and ⌃K is `deleteToEndOfLine:`.
        // ⌃K doesn't clash with the ⌃0-6 headings above — those are digits.
        KeyBinding::new("cmd-backspace", DeleteToLineStart, ctx),
        KeyBinding::new("ctrl-k", DeleteToLineEnd, ctx),
        // ⌘⇧x/u for strike/underline — ⌥ stays word motion, as above. Neither
        // collides: ⌘⇧u is a letter, where ⌘⇧↑ (SelectDocStart) is a named key.
        KeyBinding::new("cmd-shift-x", ToggleStrikethrough, ctx),
        KeyBinding::new("cmd-shift-u", ToggleUnderline, ctx),
        KeyBinding::new("cmd-shift-s", SaveAs, ctx),
        KeyBinding::new("cmd-n", NewDocument, ctx),
    ]);
}

/// The embeddable editor widget: a `leaf_core::Doc` plus gpui focus, and the
/// last painted layout cached so a mouse event can hit-test pixels back to a
/// source offset. Drop it into any gpui view with [`register_keybindings`]; it
/// renders just the editing surface and leaves window chrome, file I/O, and quit
/// to the host (the `leaf` binary is one such host).
pub struct Editor {
    focus_handle: FocusHandle,
    /// The open document, or `None` when the widget is empty (the host decides
    /// what to show in that case — the app overlays a file-open button).
    doc: Option<Doc>,
    /// The IME's composition (preedit) span in *source* bytes, while one is up.
    /// The text is really in the document — twig has no notion of provisional
    /// bytes — so this is what tells the renderer to underline it and what a
    /// following composition keystroke replaces. See `EntityInputHandler`.
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    /// Set while the right-click context menu is open, to the window position
    /// it should be anchored at. `None` hides it.
    context_menu: Option<Point<Pixels>>,
    /// Scroll offset of the document body; lets the view exceed the window.
    scroll_handle: ScrollHandle,
    // Filled by the element each paint; read by mouse handlers to hit-test.
    // `Rc` so a paint that can reuse them costs a refcount rather than a copy of
    // every shaped line in the document.
    last_rows: Rc<Vec<RowLayout>>,
    /// The geometry the last paint's table chrome was drawn from, kept with the
    /// rows it was measured against.
    last_geoms: Rc<Vec<TableGeom>>,
    /// The code blocks the last paint drew a box around, kept with the rows —
    /// which output rows each occupies, so the mouse can tell a click landed in a
    /// scrolled code block and undo its horizontal offset.
    last_code_geoms: Rc<Vec<CodeGeom>>,
    /// The block images the last paint reserved rows for, kept with the rows so a
    /// repaint that reuses them (a blink, a scroll) re-draws each raster without
    /// re-laying-out. Rides the row cache alongside [`Self::last_geoms`].
    last_image_geoms: Rc<Vec<ImageGeom>>,
    /// Decoded rasters keyed by resolved file path, so an image is read and
    /// decoded once per document session rather than every relayout. `None` marks
    /// a path that failed to load (missing, unsupported), so a broken image is
    /// retried at most once per session, not every frame. The stable `RenderImage`
    /// id also keeps gpui's sprite-atlas upload cached across frames.
    image_cache: HashMap<PathBuf, Option<Arc<RenderImage>>>,
    /// The horizontal pixel delta added to each row's text when it was painted —
    /// a code block's indent-minus-scroll, zero for ordinary rows. Parallel to
    /// [`Self::last_rows`]; the mouse subtracts it to hit-test a scrolled row.
    last_row_x: Rc<Vec<Pixels>>,
    /// What `last_rows` was built from, or `None` before the first paint.
    ///
    /// The rows are a pure function of this key, and shaping them is
    /// O(document) — 37 ms on a 1 MB file. A repaint is not an edit: the caret
    /// blinks twice a second, and scrolling, focus changes, and window
    /// activation all repaint a document that hasn't moved a byte. Every one of
    /// those used to re-shape the whole thing.
    layout_key: Option<LayoutKey>,
    /// Shapes kept from the last paint, so an edit re-shapes only the text that
    /// actually changed — see [`Shaper`].
    shape_cache: HashMap<u64, Rc<ShapedLine>>,
    /// Where each logical line wraps — see [`Shaper::breaks`].
    break_cache: HashMap<(u64, u32), Rc<Vec<usize>>>,
    /// A representative (body) line height from the last paint — enough for page
    /// up/down, which steps by viewportfuls. Exact per-row geometry rides
    /// [`Self::last_row_tops`] instead, since a heading's row is taller.
    last_line_height: Pixels,
    /// The top y of each visual row from the last paint, relative to the text
    /// origin, plus a final entry for the bottom of the last row (so it has
    /// `rows + 1` entries). Rows are no longer a uniform height — a heading's is
    /// larger — so caret placement, selection, mouse hit-testing, scrolling, and
    /// the IME rect all read their y from here rather than multiplying a row
    /// index by one line height.
    last_row_tops: Rc<Vec<Pixels>>,
    last_bounds: Option<Bounds<Pixels>>,
    /// Visual-row count from the last paint — request_layout reserves height for
    /// it, since the true (pixel-wrapped) count is only known once we've laid out.
    last_row_count: usize,
    /// The "sticky" x the caret aims for through a run of vertical moves, and the
    /// caret offset it was computed for. If the caret has since moved by any other
    /// path (typing, a horizontal key, a click), `goal_caret` no longer matches and
    /// the goal is recomputed — so we never sprinkle resets across every handler.
    goal_x: Option<Pixels>,
    goal_caret: usize,
    /// The host-overridable theme (colors, font metrics).
    style: EditorStyle,
    /// Space at the bottom of the viewport the caret must stay clear of — set by
    /// the host to the on-screen keyboard height on mobile, so the edited line is
    /// never hidden behind the keyboard. `0` on desktop.
    bottom_inset: Pixels,
    /// The modal question on screen, or `None`. Both the close guard
    /// ([`Self::confirm_close`], which every host's quit *and* window-close
    /// path goes through) and the disk-conflict checks raise one, so the two
    /// share this one piece of state rather than each host tracking its own.
    dialog: Option<Dialog>,
    /// The modal text prompt (⌘K's link destination, ⌘⇧S's file name), or
    /// `None` when none is open. `Some` both drives `render`'s prompt overlay
    /// and gates every document key/action/mouse listener off, so the prompt
    /// owns the keyboard until it closes — see the `prompt` module and `render`.
    prompt: Option<TextPrompt>,
    /// The caret's blink phase: `Some(false)` is the half-second it's hidden
    /// for. `None` means no blink loop is running (nothing is focused, so no
    /// caret paints anyway) — distinct from `Some(true)`, so a widget that has
    /// never rendered still shows a caret rather than waiting on a timer.
    blink_phase: Option<bool>,
    /// The live blink timer, *held* rather than detached: assigning a new one
    /// drops this, which cancels it. That's what keeps a run of typing — every
    /// keystroke restarts the blink — from leaving a stack of loops behind, all
    /// toggling the same caret at once.
    blink_task: Task<()>,
    /// `(caret, source len)` as of the last blink restart. Comparing against it
    /// is how `sync_blink` spots the edit or motion that has to pause the blink,
    /// without a `pause_blink()` call threaded through every action handler.
    blink_caret: Option<(usize, usize)>,
}

impl Editor {
    /// Create the widget over an optional document (`None` = empty), with the
    /// default theme. Register the key bindings once with [`register_keybindings`],
    /// size the returned entity in your view, and focus its
    /// [`Focusable::focus_handle`] to type into it. Restyle with [`Self::set_style`].
    pub fn new(cx: &mut Context<Self>, doc: Option<Doc>) -> Self {
        Editor {
            focus_handle: cx.focus_handle(),
            doc,
            marked_range: None,
            is_selecting: false,
            context_menu: None,
            scroll_handle: ScrollHandle::new(),
            last_rows: Rc::new(Vec::new()),
            last_geoms: Rc::new(Vec::new()),
            last_code_geoms: Rc::new(Vec::new()),
            last_image_geoms: Rc::new(Vec::new()),
            image_cache: HashMap::new(),
            last_row_x: Rc::new(Vec::new()),
            layout_key: None,
            shape_cache: HashMap::new(),
            break_cache: HashMap::new(),
            last_line_height: px(24.0),
            last_row_tops: Rc::new(Vec::new()),
            last_bounds: None,
            last_row_count: 0,
            goal_x: None,
            goal_caret: usize::MAX,
            style: EditorStyle::default(),
            bottom_inset: px(0.0),
            dialog: None,
            prompt: None,
            blink_phase: None,
            // No caret is focused yet, so there is nothing to blink; the first
            // `sync_blink` of a focused frame starts the real loop.
            blink_task: Task::ready(()),
            blink_caret: None,
        }
    }

    /// Set the space the caret must stay clear of at the bottom of the viewport
    /// (e.g. the on-screen keyboard height on mobile). Scrolls the caret above it.
    /// Hosts call this when the keyboard shows/hides.
    pub fn set_bottom_inset(&mut self, inset: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        if self.bottom_inset == inset {
            return;
        }
        self.bottom_inset = inset;
        self.scroll_caret_into_view();
        cx.notify();
        // The extra scroll room this inset reserves only lands in the layout when
        // the *next* frame paints — but `on_next_frame` callbacks run at the top
        // of a frame, before that frame's own paint. So the first callback still
        // sees the old `max_offset` and can't move the document end. Hop one more
        // frame: by then the inset-reserving paint is in and the re-scroll has the
        // room it needs to lift the last line clear of the keyboard.
        let editor = cx.entity();
        window.on_next_frame(move |window, _cx| {
            let editor = editor.clone();
            window.on_next_frame(move |_window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.scroll_caret_into_view();
                    cx.notify();
                });
            });
        });
    }

    /// Replace the widget's theme (colors, font) to match the host application.
    pub fn set_style(&mut self, style: EditorStyle, cx: &mut Context<Self>) {
        self.style = style;
        // The font and its size are shaped *into* the rows, so a theme change
        // that only recolours is indistinguishable here from one that changes
        // the typeface. Re-shape rather than guess.
        self.invalidate_layout();
        cx.notify();
    }

    /// Debug snapshot of caret state: `(caret, selection anchor, source len)`.
    /// Used by the iOS toolbar logging to diagnose command behaviour.
    pub fn caret_debug(&self) -> Option<(usize, Option<usize>, usize)> {
        self.doc.as_ref().map(|d| (d.caret, d.anchor, d.source.len()))
    }

    /// Verbose diagnostic string (view, caret, selection, the block the caret is
    /// in, its heading level, and the last op's status). Used by the iOS toolbar
    /// logging to see why a block command did or didn't apply.
    pub fn diag(&mut self) -> String {
        match self.doc.as_mut() {
            None => "no doc".into(),
            Some(d) => {
                // These take &mut self; compute before the immutable reads below.
                let block = d.breadcrumb();
                let heading = d.current_heading_level();
                format!(
                    "view={} caret={} anchor={:?} len={} block=[{block}] heading={heading:?} status={:?}",
                    d.view_name(),
                    d.caret,
                    d.anchor,
                    d.source.len(),
                    d.status,
                )
            }
        }
    }

    /// Run a command programmatically (native toolbar, menu, etc.), equivalent
    /// to invoking the corresponding keybound action.
    pub fn run_command(&mut self, cmd: EditorCommand, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.is_some() || self.dialog.is_some() {
            return; // a modal owns the widget until it's answered
        }
        match cmd {
            EditorCommand::ToggleBold => self.toggle_bold(&ToggleBold, window, cx),
            EditorCommand::ToggleItalic => self.toggle_italic(&ToggleItalic, window, cx),
            EditorCommand::ToggleCode => self.toggle_code(&ToggleCode, window, cx),
            EditorCommand::ToggleMark => self.toggle_mark(&ToggleMark, window, cx),
            EditorCommand::Paragraph => self.set_paragraph(&Paragraph, window, cx),
            EditorCommand::Heading1 => self.heading1(&Heading1, window, cx),
            EditorCommand::Heading2 => self.heading2(&Heading2, window, cx),
            EditorCommand::Heading3 => self.heading3(&Heading3, window, cx),
            EditorCommand::Heading4 => self.heading4(&Heading4, window, cx),
            EditorCommand::Heading5 => self.heading5(&Heading5, window, cx),
            EditorCommand::Heading6 => self.heading6(&Heading6, window, cx),
            EditorCommand::ToggleView => self.toggle_view(&ToggleView, window, cx),
            EditorCommand::Undo => self.undo(&Undo, window, cx),
            EditorCommand::Redo => self.redo(&Redo, window, cx),
            EditorCommand::Save => self.save(&Save, window, cx),
        }
        // Keep the affected line visible (above the keyboard on mobile) so the
        // result of a toolbar command is always on screen.
        self.scroll_caret_into_view();
    }

    /// Whether a document is open. The host shows its own placeholder otherwise.
    pub fn has_doc(&self) -> bool {
        self.doc.is_some()
    }

    /// Whether the open document has unsaved edits (for a host's title/close UI).
    pub fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.dirty)
    }

    /// Ask whether it's safe to close the widget right now — the one method a
    /// host's quit/window-close guard should defer to, so every embedder gets
    /// the same unsaved-changes protection instead of reimplementing it.
    ///
    /// A clean document answers `true`: close away. A dirty one answers `false`
    /// and puts the real question on screen — Save / Discard / Cancel — and the
    /// host's job is then to do *nothing* and wait: two of those three answers
    /// end in an [`EditorEvent::CloseConfirmed`], which is the host's cue to
    /// go. A host that only ever wants the clean/dirty bit can read
    /// [`Self::is_dirty`] instead and never subscribe.
    pub fn confirm_close(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.is_dirty() {
            return true;
        }
        self.dialog = Some(Dialog::Discard { then: Pending::Quit });
        cx.notify();
        false
    }

    /// Dismiss whatever modal question is up, taking no action on it — a host's
    /// Escape/Cancel handler. Returns whether there was one, so a host chaining
    /// this after [`Self::cancel_prompt`] knows if it consumed the keystroke.
    pub fn dismiss_dialog(&mut self, cx: &mut Context<Self>) -> bool {
        if self.dialog.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    /// Notice a file that has changed underneath the open document and offer the
    /// reload — the gap that otherwise ends with the next save silently
    /// clobbering whatever the other writer did.
    ///
    /// **Costs a file read and a hash** ([`Doc::disk_state`]), so this is a
    /// moment to be picked, never a per-frame question. The `leaf` binary calls
    /// it on window activation (gpui's `observe_window_activation`), which is
    /// precisely when the user is coming back from the other program that
    /// touched the file. The *other* moment that matters — immediately before a
    /// save — the widget owns itself and doesn't need a host to ask for.
    pub fn check_disk_state(&mut self, cx: &mut Context<Self>) {
        if self.dialog.is_some() || self.prompt.is_some() {
            return; // already asking something; don't stack a second question
        }
        let Some(doc) = self.doc.as_ref() else { return };
        // Only `Changed` is a question. `Missing` isn't: a save recreates the
        // file, which is what the user meant, and nagging about a file someone
        // moved would be noise. `Unreadable` leaves leaf unable to tell, and it
        // won't guess (see `DiskState`).
        if doc.disk_state() != DiskState::Changed {
            return;
        }
        self.dialog = Some(Dialog::DiskChanged);
        cx.notify();
    }

    /// The inline marks in force at the caret — what a host's header or toolbar
    /// lights up (Bold reads active when the caret sits in bold text).
    ///
    /// `&mut self` because leaf-core's answer is an AST query, not a cached
    /// flag. It *is* a per-frame question, unlike [`Self::check_disk_state`]:
    /// `InlineMarks` is a `Copy` bitset precisely so a toolbar can ask it every
    /// frame without allocating (see its doc comment).
    pub fn active_marks(&mut self) -> InlineMarks {
        self.doc
            .as_mut()
            .map(|d| d.active_inline_marks())
            .unwrap_or_default()
    }

    /// The open document's file name, or empty when none is open.
    pub fn file_name(&self) -> String {
        self.doc.as_ref().map(|d| d.file_name()).unwrap_or_default()
    }

    /// The active view's label (`source` / `wysiwyg`), or empty when no document.
    pub fn view_label(&self) -> &'static str {
        self.doc.as_ref().map(|d| d.view_name()).unwrap_or("")
    }

    /// Open a document into the widget (e.g. after the host's file picker).
    pub fn set_doc(&mut self, doc: Doc, cx: &mut Context<Self>) {
        self.doc = Some(doc);
        self.goal_x = None;
        // A revision only counts edits *within* one document — every freshly
        // opened one starts at zero. So the key for a new document can equal the
        // key for the old one, and without this the editor would open a file and
        // paint the previous one's text. The revision can't see this; only the
        // swap itself can.
        self.invalidate_layout();
        cx.notify();
    }

    /// Drop the shaped rows, for a change the layout key can't describe.
    ///
    /// The key covers what the *document* contributes. Everything else the rows
    /// depend on — which document it is, what font it's in — changes underneath
    /// it, so those paths say so here.
    fn invalidate_layout(&mut self) {
        self.layout_key = None;
        // The shapes and the breaks both carry the font they were measured
        // with, so they go too.
        self.shape_cache.clear();
        self.break_cache.clear();
    }

    /// Persist the open document to its path, unconditionally and with no
    /// questions asked — no Save As for an untitled document, no disk-conflict
    /// check. A host that owns saving outright can rebind `Save` and call this;
    /// the widget's own ⌘S goes through `try_save`, which asks both.
    pub fn save_document(&mut self) {
        if let Some(doc) = self.doc.as_mut() {
            doc.save();
        }
    }

    // ── save / reload / new, and the questions they have to ask first ────────

    /// ⌘S, and everything between the keystroke and the bytes landing.
    ///
    /// Two things can stand between them, and both need a human: a document
    /// with no name yet can't be written anywhere (leaf-core won't invent a
    /// path — `Doc::save` just says "untitled — save as…"), and a file that
    /// someone else has written since we read it would be silently clobbered.
    /// `then` is what happens after the bytes are down, for the callers that
    /// are on their way somewhere (the quit dialog's Save).
    fn try_save(&mut self, then: Pending, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_ref() else { return };
        if doc.is_untitled() {
            return self.open_save_as(then, window, cx);
        }
        // The one moment a stale answer costs someone their work, and the
        // reason `disk_state`'s file read is affordable here: it's one read per
        // ⌘S, not per frame. A clean document has nothing of its own to lose,
        // so it isn't a conflict — `check_disk_state` offers it the reload.
        if doc.dirty && doc.disk_state() == DiskState::Changed {
            self.dialog = Some(Dialog::Overwrite { then });
            cx.notify();
            return;
        }
        self.commit_save(then, cx);
    }

    /// Write, and go wherever the save was headed — but only if it landed. A
    /// failed write leaves `dirty` set and a `save failed: …` status, and
    /// quitting on that would discard the very work the dialog's Save promised
    /// to keep.
    fn commit_save(&mut self, then: Pending, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.save();
        let saved = !doc.dirty;
        self.dialog = None;
        if saved {
            self.finish(then, cx);
        }
        cx.notify();
    }

    /// Open the Save As prompt, prefilled with the document's current path so
    /// that saving a copy elsewhere means editing the name rather than retyping
    /// it — the same prefill rule ⌘K's link prompt follows. An untitled
    /// document has nothing to offer, and starts blank.
    fn open_save_as(&mut self, then: Pending, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_ref() else { return };
        let initial = if doc.is_untitled() {
            String::new()
        } else {
            doc.path.display().to_string()
        };
        // The prompt takes the dialog's place: it's the same question, one step
        // on, and two modals at once would be answering neither.
        self.dialog = None;
        self.open_prompt("Save as", initial, PromptAction::SaveAs { then }, window, cx);
    }

    /// A confirmed Save As: write to the typed path, and go on to whatever the
    /// Save As was in the way of. `Doc::save_as` *moves* the document to the new
    /// path, so every later ⌘S writes there too.
    fn commit_save_as(&mut self, path: &str, then: Pending, cx: &mut Context<Self>) {
        let path = path.trim();
        let Some(doc) = self.doc.as_mut() else { return };
        if path.is_empty() {
            // An empty name is not a path. `Doc::save_as` would take it and
            // fail at the filesystem; say what happened instead.
            doc.status = Some("save as: no file name".into());
            cx.notify();
            return;
        }
        doc.save_as(PathBuf::from(path));
        let saved = !doc.dirty;
        if saved {
            self.finish(then, cx);
        }
        cx.notify();
    }

    /// Throw the buffer away for what's on disk. Only ever reached from a
    /// dialog: `Doc::reload` discards unsaved edits *and* the undo history
    /// without checking anything, on the understanding that the frontend asked
    /// first (see its doc comment). This is that asking.
    fn reload_document(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.reload();
        self.dialog = None;
        // The caret moved to wherever clamping put it, by no route the sticky
        // goal x knows about, and the rows under it are different text now.
        self.goal_x = None;
        self.scroll_caret_into_view();
        cx.notify();
    }

    /// Do the thing the dialog was in front of, now that the document has been
    /// saved or deliberately abandoned.
    fn finish(&mut self, then: Pending, cx: &mut Context<Self>) {
        match then {
            Pending::Nothing => {}
            // The widget can't quit — the process is the host's — so this is
            // the one place the two are stitched together.
            Pending::Quit => cx.emit(EditorEvent::CloseConfirmed),
            Pending::NewDoc => self.open_blank(cx),
        }
    }

    /// Replace the document with a fresh untitled one.
    fn open_blank(&mut self, cx: &mut Context<Self>) {
        match Doc::blank() {
            Ok(doc) => self.set_doc(doc, cx),
            Err(e) => eprintln!("leaf: {e}"),
        }
    }

    // ── keyboard actions → leaf-core Doc ops ────────────────────────────────
    // Every handler is a no-op until a document is open.
    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_left(false);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_right(false);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_line(-1, false, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_line(1, false, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_left(true);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_right(true);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_line(-1, true, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_line(1, true, cx);
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to_line_edge(false, false, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to_line_edge(true, false, cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to_line_edge(false, true, cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to_line_edge(true, true, cx);
    }

    // ── visual-line motion (the GUI wraps at pixel width, so ↑/↓ and Home/End
    //    move by *visual* row, computed from the last painted layout — leaf-core's
    //    row model is one-per-paragraph and can't see the pixel wrap) ───────────

    /// Move the caret one visual row up (`dir < 0`) or down, keeping a sticky
    /// goal x across the run. Falls back to the model's paragraph-level move only
    /// before the first paint, when there's no cached layout to read.
    fn move_line(&mut self, dir: i32, extend: bool, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        match self.vertical_target(dir) {
            Some((off, goal)) => {
                self.goal_x = Some(goal);
                self.goal_caret = off;
                self.doc.as_mut().unwrap().place_caret(off, extend);
            }
            None => {
                let doc = self.doc.as_mut().unwrap();
                if dir < 0 {
                    doc.move_up(extend);
                } else {
                    doc.move_down(extend);
                }
            }
        }
        self.scroll_caret_into_view();
        cx.notify();
    }

    /// The source offset one visual row away in `dir`, plus the goal x used — or
    /// `None` if there's no cached layout yet.
    fn vertical_target(&self, dir: i32) -> Option<(usize, Pixels)> {
        let doc = self.doc.as_ref()?;
        let rows = &self.last_rows;
        if rows.is_empty() {
            return None;
        }
        let (r, gi) = locate_caret(rows, doc.caret);
        // Reuse the sticky x only if the caret hasn't moved by some other path.
        let x = match self.goal_x {
            Some(x) if self.goal_caret == doc.caret => x,
            _ => rows[r].x_at(gi),
        };
        // Step in `dir`, stepping *over* any row the caret can't rest on — a
        // block-gap separator is painted for paragraph spacing but carries no
        // caret stop, the pixel-wrap analogue of the decoration rows `leaf-core`'s
        // `navigable_above`/`navigable_below` skip for the TUI. Without this a
        // Down into the gap would land on its only nearby stop — the line above —
        // and `place_caret`'s snap would pin the caret one row short of the next
        // paragraph. Running off either end means there's no navigable row that
        // way; fall back (`None`) to the model's edge move (Cocoa's jump to the
        // document's start/end).
        let mut tr = r as i32 + dir;
        while (0..rows.len() as i32).contains(&tr) {
            let cand = tr as usize;
            if self.row_is_navigable(cand) {
                return Some((rows[cand].src_at_index(rows[cand].index_for_x(x)), x));
            }
            tr += dir;
        }
        None
    }

    /// Whether the caret can rest on visual row `r`. Every source-view row can;
    /// in WYSIWYG a block-gap separator can't — it's drawn for spacing but holds
    /// no caret stop, so its `end_src` isn't a stop and it carries no glyph whose
    /// source offset is one (an *empty paragraph*, by contrast, is a real stop and
    /// stays navigable). Read straight off the visual map's stop table so it can't
    /// drift from what `place_caret` will snap to.
    fn row_is_navigable(&self, r: usize) -> bool {
        let Some(doc) = self.doc.as_ref() else {
            return true;
        };
        if doc.view != View::Wysiwyg {
            return true;
        }
        let row = &self.last_rows[r];
        doc.vmap.is_stop(row.end_src) || row.char_srcs.iter().any(|&s| doc.vmap.is_stop(s))
    }

    /// Move the caret to the start (`to_end = false`) or end of its *visual* row.
    fn move_to_line_edge(&mut self, to_end: bool, extend: bool, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        let target = {
            let rows = &self.last_rows;
            let caret = self.doc.as_ref().unwrap().caret;
            if rows.is_empty() {
                None
            } else {
                let (r, _) = locate_caret(rows, caret);
                let row = &rows[r];
                Some(if to_end {
                    row.end_src
                } else {
                    row.char_srcs.first().copied().unwrap_or(row.end_src)
                })
            }
        };
        self.goal_x = None;
        match target {
            Some(off) => self.doc.as_mut().unwrap().place_caret(off, extend),
            None => {
                let doc = self.doc.as_mut().unwrap();
                if to_end {
                    doc.move_end(extend);
                } else {
                    doc.move_home(extend);
                }
            }
        }
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.backspace();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.delete_forward();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.newline();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.indent();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.outdent();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn delete_to_line_start(&mut self, _: &DeleteToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.delete_to_line_start();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn delete_to_line_end(&mut self, _: &DeleteToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.delete_to_line_end();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn toggle_bold(&mut self, _: &ToggleBold, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle(InlineKind::Strong);
        cx.notify();
    }
    fn toggle_italic(&mut self, _: &ToggleItalic, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle(InlineKind::Emph);
        cx.notify();
    }
    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.try_save(Pending::Nothing, window, cx);
    }
    /// ⌘⇧S: name the file, whether or not it already has a name.
    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.open_save_as(Pending::Nothing, window, cx);
    }
    /// ⌘N: a fresh untitled document. leaf is one document in one window, so
    /// this *replaces* what's open — which makes it a discard, and it goes
    /// through the same guard as a quit rather than dropping the work quietly.
    fn new_document(&mut self, _: &NewDocument, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_dirty() {
            self.dialog = Some(Dialog::Discard { then: Pending::NewDoc });
            cx.notify();
            return;
        }
        self.open_blank(cx);
    }
    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_doc(false, false, cx);
    }
    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_doc(true, false, cx);
    }
    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_doc(false, true, cx);
    }
    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_doc(true, true, cx);
    }
    fn jump_doc(&mut self, to_end: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        self.goal_x = None;
        if to_end {
            doc.move_doc_end(extend);
        } else {
            doc.move_doc_start(extend);
        }
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_page(-1, false, cx);
    }
    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_page(1, false, cx);
    }
    fn select_page_up(&mut self, _: &SelectPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_page(-1, true, cx);
    }
    fn select_page_down(&mut self, _: &SelectPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_page(1, true, cx);
    }
    /// Move the caret a screenful of visual rows in `dir`, reusing the per-row
    /// vertical target so it rides the pixel wrap exactly like ↑/↓.
    fn move_page(&mut self, dir: i32, extend: bool, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        let rows = self.page_rows();
        for _ in 0..rows {
            let Some((off, goal)) = self.vertical_target(dir) else { break };
            self.goal_x = Some(goal);
            self.goal_caret = off;
            self.doc.as_mut().unwrap().place_caret(off, extend);
        }
        self.scroll_caret_into_view();
        cx.notify();
    }
    /// Visible rows in the body viewport, minus one for overlap — the page step.
    fn page_rows(&self) -> usize {
        let viewport = f32::from(self.scroll_handle.bounds().size.height);
        let line = f32::from(self.last_line_height).max(1.0);
        ((viewport / line) as usize).saturating_sub(1).max(1)
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.undo();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.redo();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn toggle_view(&mut self, _: &ToggleView, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle_view();
        cx.notify();
    }
    fn move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_word_left(false);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_word_right(false);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_word_left(true);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_word_right(true);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn delete_word_back(&mut self, _: &DeleteWordBack, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.delete_word_back();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.delete_word_forward();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.select_all();
        cx.notify();
    }
    fn toggle_code(&mut self, _: &ToggleCode, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle(InlineKind::Verbatim);
        cx.notify();
    }
    fn toggle_mark(&mut self, _: &ToggleMark, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle(InlineKind::Mark);
        cx.notify();
    }
    /// ⌘⇧X. twig models strikethrough as a `Delete` inline — text marked as
    /// struck *out*, in the edit-tracking sense the name comes from.
    fn toggle_strikethrough(&mut self, _: &ToggleStrikethrough, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle(InlineKind::Delete);
        cx.notify();
    }
    /// ⌘⇧U — `Insert`, the other half of twig's edit-tracking pair.
    fn toggle_underline(&mut self, _: &ToggleUnderline, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle(InlineKind::Insert);
        cx.notify();
    }
    fn set_paragraph(&mut self, _: &Paragraph, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.set_block(BlockKind::Paragraph);
        cx.notify();
    }
    /// Shared body for the six `Heading{1..6}` actions below — each is a
    /// distinct zero-sized action type (gpui binds keys to types, not values),
    /// so the handlers are thin wrappers over this.
    fn set_heading(&mut self, level: u32, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        // Toggle: applying a heading a line already has reverts it to a paragraph
        // (matches bold/italic/code, and lets ⌃1/H1 undo itself).
        doc.toggle_heading(level);
        cx.notify();
    }
    fn heading1(&mut self, _: &Heading1, _: &mut Window, cx: &mut Context<Self>) {
        self.set_heading(1, cx);
    }
    fn heading2(&mut self, _: &Heading2, _: &mut Window, cx: &mut Context<Self>) {
        self.set_heading(2, cx);
    }
    fn heading3(&mut self, _: &Heading3, _: &mut Window, cx: &mut Context<Self>) {
        self.set_heading(3, cx);
    }
    fn heading4(&mut self, _: &Heading4, _: &mut Window, cx: &mut Context<Self>) {
        self.set_heading(4, cx);
    }
    fn heading5(&mut self, _: &Heading5, _: &mut Window, cx: &mut Context<Self>) {
        self.set_heading(5, cx);
    }
    fn heading6(&mut self, _: &Heading6, _: &mut Window, cx: &mut Context<Self>) {
        self.set_heading(6, cx);
    }
    fn toggle_blockquote(&mut self, _: &ToggleBlockquote, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle_blockquote();
        cx.notify();
    }
    fn toggle_bullet_list(&mut self, _: &ToggleBulletList, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle_list(false);
        cx.notify();
    }
    fn toggle_ordered_list(
        &mut self,
        _: &ToggleOrderedList,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.toggle_list(true);
        cx.notify();
    }
    /// ⌘K: open the link prompt, prefilled with the destination of the link
    /// the caret already sits in (if any) so re-pointing a link means editing
    /// its URL rather than retyping it from scratch.
    fn insert_link(&mut self, _: &InsertLink, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        let initial = doc.link_destination_at_caret().unwrap_or_default();
        self.open_prompt("Link destination", initial, PromptAction::Link, window, cx);
    }

    /// ⌘⇧L: set the language of the fenced code block the caret is in, prefilled
    /// with its current language — the code-block analogue of ⌘K, editing the
    /// fence's info string through a prompt rather than exposing fence markup as
    /// an editable row. A no-op when the caret is in no fenced block.
    fn set_language(&mut self, _: &SetLanguage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        if !doc.caret_in_fenced_code() {
            return;
        }
        let initial = doc.code_language_at_caret().unwrap_or_default();
        self.open_prompt("Code language", initial, PromptAction::SetLanguage, window, cx);
    }
    // ── clipboard (arboard, not gpui's — see `set_clipboard`) ───────────────

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(text) = doc.selected_text().map(str::to_string) else { return };
        let html = doc.selection_html();
        set_clipboard(text, html, cx);
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(text) = doc.selected_text().map(str::to_string) else { return };
        let html = doc.selection_html();
        set_clipboard(text, html, cx);
        self.doc.as_mut().unwrap().backspace();
        self.scroll_caret_into_view();
        cx.notify();
    }

    /// ⌘V: the rich flavor where the pasteboard has one, the plain flavor
    /// otherwise — including when the HTML won't convert to anything worth
    /// pasting (see `leaf_core::html`), because the two flavors describe the
    /// same content and the plain one always exists.
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let pasted = get_clipboard_html(cx)
            .is_some_and(|html| self.doc.as_mut().is_some_and(|doc| doc.paste_html(&html)));
        if pasted {
            self.scroll_caret_into_view();
            cx.notify();
            return;
        }
        self.paste_as_plain_text(&PasteAsPlainText, window, cx);
    }

    /// ⌘⇧V: the plain flavor, whatever else the pasteboard carries — the escape
    /// hatch for pasting the *source* of something rich.
    fn paste_as_plain_text(&mut self, _: &PasteAsPlainText, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = get_clipboard_text(cx) else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        doc.paste(&text);
        self.scroll_caret_into_view();
        cx.notify();
    }

    // ── modal text prompt ────────────────────────────────────────────────────
    // A minimal, reusable single-line input (see the `prompt` module): opened
    // over a label/initial value/[`PromptAction`], it owns the keyboard until
    // Enter or Esc closes it. `render` gates every document key binding, mouse
    // handler, and (via `EntityInputHandler` simply losing focus) IME hookup
    // behind `self.prompt.is_none()`, so none of them see a keystroke meant
    // for the prompt.

    /// Open the prompt over `value` (already the right starting text — prefill
    /// or blank is the caller's call), focused and ready to type into.
    fn open_prompt(
        &mut self,
        label: impl Into<SharedString>,
        value: String,
        action: PromptAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = TextPrompt::new(label, value, action, cx.focus_handle());
        prompt.focus_handle.focus(window, cx);
        self.prompt = Some(prompt);
        cx.notify();
    }

    /// Enter: hand the prompt's collected text to whatever it was opened for,
    /// then return focus (and the keyboard) to the document.
    fn confirm_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else { return };
        self.focus_handle.focus(window, cx);
        match prompt.action {
            PromptAction::Link => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.insert_link(&prompt.value);
                }
            }
            PromptAction::SetLanguage => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.set_code_language(&prompt.value);
                }
            }
            PromptAction::SaveAs { then } => self.commit_save_as(&prompt.value, then, cx),
        }
        self.scroll_caret_into_view();
        cx.notify();
    }

    /// Dismiss an open prompt without acting on it — Esc, or a host's own
    /// Escape guard falling back to this first (see `crates/leaf`'s `LeafApp::cancel`:
    /// its unconditional ⎋⇒Cancel binding resolves before this widget's own
    /// keystroke handling ever runs, so a host embedding a modal-aware `Editor`
    /// needs to ask it first). Returns whether a prompt was actually open, so
    /// that caller knows whether it just handled the keystroke.
    pub fn cancel_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.prompt.take().is_none() {
            return false;
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
        true
    }

    /// The prompt's raw key handling — Enter/Escape/Backspace by name, anything
    /// else by its resolved `key_char`. Raw rather than gpui actions/keybindings
    /// because the prompt has no fixed key context of its own to bind against;
    /// reading straight off the keystroke is simpler than inventing one action
    /// type per key for a single-purpose overlay. Fires only while the prompt
    /// holds focus (this listener lives on the prompt's own element).
    fn prompt_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.prompt.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "enter" => self.confirm_prompt(window, cx),
            "escape" => {
                self.cancel_prompt(window, cx);
            }
            "backspace" => {
                self.prompt.as_mut().unwrap().backspace();
                cx.notify();
            }
            _ => {
                // `key_char` is `None` for bare navigation/function keys and
                // for anything chorded with ⌘/⌃, so this naturally ignores
                // everything but genuine typed text.
                if let Some(ch) = event.keystroke.key_char.as_deref().filter(|c| !c.is_empty()) {
                    self.prompt.as_mut().unwrap().insert(ch);
                    cx.notify();
                }
            }
        }
    }

    /// Scroll the document body, if needed, so the caret's row is visible after
    /// a keyboard motion or edit. Works in window-space from the last painted
    /// geometry: `last_bounds` is the text's top (already reflecting the current
    /// scroll) and `scroll_handle.bounds()` is the viewport. The delta between
    /// them is a pure translation, so applying it to the current offset is exact
    /// even though both come from the previous frame.
    fn scroll_caret_into_view(&mut self) {
        let Some(doc) = self.doc.as_ref() else { return };
        let Some(text_bounds) = self.last_bounds else {
            return;
        };
        let view = self.scroll_handle.bounds();
        if view.size.height <= px(0.0) {
            return; // not laid out yet
        }
        // The caret's *visual* row — located against the painted rows, since the
        // pixel wrap means one paragraph can span several rows (caret_pos() would
        // give the paragraph index instead).
        let row = if self.last_rows.is_empty() {
            0
        } else {
            locate_caret(&self.last_rows, doc.caret).0
        };
        // Its top and height come from the cumulative tops (a heading's row is
        // taller); fall back to the body line height before the first paint.
        let tops = &self.last_row_tops;
        let (caret_top, caret_h) = match (tops.get(row), tops.get(row + 1)) {
            (Some(&t), Some(&b)) => (text_bounds.top() + t, b - t),
            _ => (text_bounds.top(), self.last_line_height),
        };
        let caret_bottom = caret_top + caret_h;

        // The keyboard (or any host inset) covers the bottom of the viewport, so
        // the caret must stay above `bottom() - bottom_inset`, not `bottom()`.
        let visible_bottom = view.bottom() - self.bottom_inset;

        let mut offset = self.scroll_handle.offset();
        if caret_top < view.top() {
            offset.y += view.top() - caret_top;
        } else if caret_bottom > visible_bottom {
            offset.y -= caret_bottom - visible_bottom;
        } else {
            return;
        }
        offset.y = offset.y.clamp(-self.scroll_handle.max_offset().y, px(0.0));
        self.scroll_handle.set_offset(offset);
    }

    // ── mouse ───────────────────────────────────────────────────────────────
    /// Left click: `click_count` (from gpui, which tracks the OS's double/
    /// triple-click timing) picks single caret placement, double-click word
    /// select, or triple-click line select. A click while the context menu is
    /// open just dismisses it (a menu item click never reaches here — it stops
    /// propagation in `context_menu_item`).
    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        if self.context_menu.take().is_some() {
            cx.notify();
            return;
        }
        self.is_selecting = true;
        let off = self.offset_for_position(ev.position);
        let Some(doc) = self.doc.as_mut() else { return };
        match ev.click_count {
            1 => doc.place_caret(off, ev.modifiers.shift),
            2 => doc.select_word_at(off),
            // Triple-click (or more): select the whole enclosing paragraph. This
            // reads the block's span from the AST, so it selects the entire
            // logical paragraph even when it soft-wraps across several rows.
            _ => doc.select_block_at(off),
        }
        // Lift the tapped line into view — above the keyboard on mobile — so a tap
        // near the bottom (keyboard already up) isn't left hidden behind it.
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }
    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let off = self.offset_for_position(ev.position);
            if let Some(doc) = self.doc.as_mut() {
                doc.place_caret(off, true);
            }
            cx.notify();
        }
    }

    /// Right click: place the caret (unless the click landed inside an
    /// existing selection, in which case Cut/Copy should act on it), then open
    /// the context menu anchored at the click.
    fn on_right_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let off = self.offset_for_position(ev.position);
        let Some(doc) = self.doc.as_mut() else { return };
        let inside_selection = doc.selection().is_some_and(|(s, e)| off >= s && off < e);
        if !inside_selection {
            doc.place_caret(off, false);
        }
        self.context_menu = Some(ev.position);
        cx.notify();
    }

    // ── context menu ─────────────────────────────────────────────────────────
    /// One clickable row of the right-click menu. Performs `on_click`, closes
    /// the menu, and — critically — stops propagation so the same click
    /// doesn't also fall through to `on_mouse_down` on the document body
    /// underneath (which would otherwise re-place the caret at the menu's
    /// screen position right after, say, a paste).
    fn context_menu_item(
        label: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(label)
            .px_3()
            .py_1()
            .cursor(CursorStyle::PointingHand)
            .hover(|s| s.bg(gpui::rgb(0xe4e4e4)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |editor, _: &MouseDownEvent, window, cx| {
                    on_click(editor, window, cx);
                    editor.context_menu = None;
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            .child(label)
    }

    /// The right-click menu itself: Cut / Copy / Paste / Select All, wired to
    /// the same ops as their keybindings. `anchored`/`deferred` paint it above
    /// the document (and, since deferred draws are hit-tested before their
    /// ancestors, its clicks are seen first too) at the window position the
    /// right-click landed on.
    fn render_context_menu(pos: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement {
        deferred(
            anchored().position(pos).snap_to_window().child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(140.0))
                    .py_1()
                    .bg(gpui::white())
                    .rounded_md()
                    .shadow_lg()
                    .border_1()
                    .border_color(gpui::rgb(0xd0d0d0))
                    .text_color(gpui::rgb(0x1e1e1e))
                    .child(Self::context_menu_item("Cut", cx, |e, w, cx| {
                        e.cut(&Cut, w, cx)
                    }))
                    .child(Self::context_menu_item("Copy", cx, |e, w, cx| {
                        e.copy(&Copy, w, cx)
                    }))
                    .child(Self::context_menu_item("Paste", cx, |e, w, cx| {
                        e.paste(&Paste, w, cx)
                    }))
                    .child(Self::context_menu_item("Select All", cx, |e, w, cx| {
                        e.select_all(&SelectAll, w, cx)
                    })),
            ),
        )
        .with_priority(1)
    }

    /// The modal prompt itself: label, typed text split around a plain caret
    /// bar (no glyph shaping — this is a single line of plain, unstyled text,
    /// not the document's rich WYSIWYG surface, so it doesn't need one). Fixed
    /// near the top of the editor rather than centered, since this only has
    /// `cx`, not the window bounds a true centered dialog would need.
    fn render_prompt(prompt: &TextPrompt, cx: &mut Context<Self>) -> impl IntoElement {
        let before = prompt.value[..prompt.caret].to_string();
        let after = prompt.value[prompt.caret..].to_string();
        deferred(
            anchored().position(point(px(24.0), px(24.0))).snap_to_window().child(
                div()
                    .id("text-prompt")
                    .track_focus(&prompt.focus_handle)
                    .on_key_down(cx.listener(Self::prompt_key_down))
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w(px(320.0))
                    .px_3()
                    .py_2()
                    .bg(gpui::white())
                    .rounded_md()
                    .shadow_lg()
                    .border_1()
                    .border_color(gpui::rgb(0xd0d0d0))
                    .text_color(gpui::rgb(0x1e1e1e))
                    .child(prompt.label.clone())
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .child(before)
                            .child(div().w(px(2.0)).h(px(16.0)).bg(gpui::blue()))
                            .child(after),
                    ),
            ),
        )
        .with_priority(1)
    }

    // ── caret blink ──────────────────────────────────────────────────────────

    /// Keep the blink in step with the focus and the caret.
    ///
    /// Called from `render`, which is the one place every path through the
    /// widget already converges on: an action that edits or moves the caret
    /// ends in `cx.notify()`, and gpui renders before it paints. So a run of
    /// typing restarts the blink — leaving the caret solid, as every editor
    /// does — without a `pause_blink()` call threaded through all thirty-odd
    /// handlers, each of which would be a place to forget one.
    fn sync_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Unfocused, no caret paints at all, and a timer still notifying twice
        // a second would re-render the widget forever for nothing. Dropping the
        // task cancels it; the next focused frame starts a fresh one.
        if !self.focus_handle.is_focused(window) {
            self.blink_task = Task::ready(());
            self.blink_phase = None;
            self.blink_caret = None;
            return;
        }
        // `(caret, len)` catches every motion and every edit — including one
        // that rewrites text without moving the caret. It deliberately misses a
        // bare format toggle (⌘B on a selection), which isn't typing and has no
        // blink to pause.
        let caret = self.doc.as_ref().map(|d| (d.caret, d.source.len()));
        if self.blink_phase.is_some() && self.blink_caret == caret {
            return; // nothing moved — let the running loop keep its phase
        }
        self.blink_caret = caret;
        self.blink_phase = Some(true);
        self.blink_task = Self::spawn_blink(cx);
    }

    /// The blink loop itself. Assigning the returned task to `blink_task` drops
    /// whatever was there, which cancels it — see that field.
    fn spawn_blink(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |editor, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                // `Err` is the entity going away — the widget is gone, so is
                // the loop.
                let alive = editor.update(cx, |editor, cx| {
                    if let Some(on) = editor.blink_phase {
                        editor.blink_phase = Some(!on);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    // ── modal dialog ─────────────────────────────────────────────────────────

    /// One button of a dialog. Same click plumbing as `context_menu_item` —
    /// including the `stop_propagation` that keeps the click off the document
    /// underneath — with a button's look instead of a menu row's.
    fn dialog_button(
        label: &'static str,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(label)
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(gpui::rgb(0xd0d0d0))
            .bg(gpui::rgb(0xf6f6f6))
            .cursor(CursorStyle::PointingHand)
            .hover(|s| s.bg(gpui::rgb(0xe4e4e4)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |editor, _: &MouseDownEvent, window, cx| {
                    on_click(editor, window, cx);
                    cx.stop_propagation();
                }),
            )
            .child(label)
    }

    /// The dialog: the question, and the buttons that are the only way to
    /// answer it. `deferred`/`anchored` for the same reasons `render_context_menu`
    /// uses them (paint above the document, get hit-tested before it), at a
    /// higher priority so it sits above the menu and the prompt too. Parked near
    /// the top rather than centered for the reason `render_prompt` gives: there
    /// are no window bounds here to center against.
    fn render_dialog(
        dialog: Dialog,
        name: String,
        dirty: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (question, buttons) = match dialog {
            Dialog::Discard { then } => {
                // The same three choices either way; only the verb differs, and
                // it names what actually happens next.
                let (save, discard) = match then {
                    Pending::Quit => ("Save and quit", "Discard and quit"),
                    _ => ("Save", "Discard"),
                };
                (
                    format!("{name} has unsaved changes."),
                    vec![
                        Self::dialog_button(save, cx, move |e, w, cx| e.try_save(then, w, cx))
                            .into_any_element(),
                        Self::dialog_button(discard, cx, move |e, _, cx| {
                            e.dialog = None;
                            e.finish(then, cx);
                            cx.notify();
                        })
                        .into_any_element(),
                        Self::dialog_button("Cancel", cx, |e, _, cx| {
                            e.dismiss_dialog(cx);
                        })
                        .into_any_element(),
                    ],
                )
            }
            Dialog::Overwrite { then } => (
                format!("{name} has changed on disk since you opened it. Saving overwrites those changes."),
                vec![
                    Self::dialog_button("Overwrite", cx, move |e, _, cx| e.commit_save(then, cx))
                        .into_any_element(),
                    // Reload drops `then` on purpose: whatever this save was on
                    // the way to, someone choosing to throw their own edits away
                    // for the file's has plainly stopped to look at it.
                    Self::dialog_button("Reload, discarding my edits", cx, |e, _, cx| {
                        e.reload_document(cx)
                    })
                    .into_any_element(),
                    Self::dialog_button("Cancel", cx, |e, _, cx| {
                        e.dismiss_dialog(cx);
                    })
                    .into_any_element(),
                ],
            ),
            Dialog::DiskChanged => (
                if dirty {
                    format!("{name} has changed on disk, and you have unsaved changes of your own. Reloading discards yours.")
                } else {
                    format!("{name} has changed on disk.")
                },
                vec![
                    Self::dialog_button("Reload", cx, |e, _, cx| e.reload_document(cx))
                        .into_any_element(),
                    Self::dialog_button("Keep mine", cx, |e, _, cx| {
                        e.dismiss_dialog(cx);
                    })
                    .into_any_element(),
                ],
            ),
        };
        deferred(
            anchored().position(point(px(24.0), px(24.0))).snap_to_window().child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(440.0))
                    .px_4()
                    .py_3()
                    .bg(gpui::white())
                    .rounded_md()
                    .shadow_lg()
                    .border_1()
                    .border_color(gpui::rgb(0xd0d0d0))
                    .text_color(gpui::rgb(0x1e1e1e))
                    .child(question)
                    .child(div().flex().gap_2().children(buttons)),
            ),
        )
        .with_priority(2)
    }

    /// Hit-test a pixel position to a *source* byte offset, reusing the cached
    /// per-row layout: pick the row by `y`, the character within it by `x`, then
    /// map that character back to the source byte it came from. Because each
    /// `RowLayout` carries per-character source offsets, this is identical for a
    /// source line and a hidden-delimiter WYSIWYG row.
    fn offset_for_position(&self, pos: Point<Pixels>) -> usize {
        let caret = self.doc.as_ref().map(|d| d.caret).unwrap_or(0);
        let Some(bounds) = self.last_bounds else {
            return caret;
        };
        if self.last_rows.is_empty() {
            return 0;
        }
        // Rows are variable height, so the row a click lands in is read from the
        // cumulative tops rather than by dividing by one line height.
        let rel_y = (pos.y - bounds.top()).max(px(0.0));
        let r = row_at_y(&self.last_row_tops, rel_y).min(self.last_rows.len() - 1);
        let row = &self.last_rows[r];
        // Undo the row's paint-time x shift (a code block's indent-minus-scroll)
        // so the click maps against the same coordinates the text was drawn in.
        let dx = self.last_row_x.get(r).copied().unwrap_or(px(0.0));
        row.src_at_index(row.index_for_x(pos.x - bounds.left() - dx))
    }

    // ── UTF-8 (leaf-core) ⇄ UTF-16 (gpui input) ─────────────────────────────
    // Thin wrappers over the free functions below, which are split out (the way
    // `locate_caret_core` is) to be unit-testable without a live document.
    fn offset_from_utf16(&self, target: usize) -> usize {
        self.doc
            .as_ref()
            .map_or(0, |d| utf16_to_utf8(&d.source, target))
    }
    fn offset_to_utf16(&self, target: usize) -> usize {
        self.doc
            .as_ref()
            .map_or(0, |d| utf8_to_utf16(&d.source, target))
    }
    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }
    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }

    fn selection_range(&self) -> Range<usize> {
        let Some(doc) = self.doc.as_ref() else {
            return 0..0;
        };
        doc.selection()
            .map(|(s, e)| s..e)
            .unwrap_or(doc.caret..doc.caret)
    }
}

// gpui routes typed text and IME through this handler; each edit becomes one of
// leaf-core's (and thus twig's) offset-addressed ops.
impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual.replace(self.range_to_utf16(&range));
        Some(self.doc.as_ref()?.source[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let doc = self.doc.as_ref()?;
        let sel = self.selection_range();
        let reversed = doc.anchor.is_some_and(|a| doc.caret < a);
        Some(UTF16Selection {
            range: self.range_to_utf16(&sel),
            reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
        // The IME dropping its composition without committing it: the bytes it
        // already spliced stay, but the run they belong to is closed, so the next
        // composition doesn't undo together with this one.
        if let Some(doc) = self.doc.as_mut() {
            doc.end_composition();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // `typed` separates ordinary keystrokes from a replacement aimed at a
        // range someone else chose. Both end up splicing over the same bytes, but
        // only one of them is a *run* the user expects to undo in one press.
        let (range, typed) = match (range_utf16, self.marked_range.clone()) {
            // Committing a composition (the IME's `insertText:` after a run of
            // `setMarkedText:`). macOS reports this replacement range relative
            // to the marked region, so it isn't an offset into this document at
            // all — and the region it means is exactly the composition we're
            // replacing. Taking it as absolute splices the finished word over
            // whatever happens to live at that offset instead.
            (Some(_), Some(marked)) | (None, Some(marked)) => (marked, false),
            // No composition: an absolute range, from something like the
            // Accessibility Keyboard's word completion. A finished word arrives
            // whole and is its own step, not part of a run.
            (Some(r), None) => (self.range_from_utf16(&r), false),
            // Plain typing: gpui names no range, so the target is just the
            // selection — which is the one `insert` resolves for itself.
            (None, None) => (self.selection_range(), true),
        };
        // A commit that ends a composition is that composition's last step, not a
        // separate edit: folding it into the run is what makes the finished word
        // undo in one press rather than unspooling back through its own reading.
        if let Some(doc) = self.doc.as_mut() {
            if self.marked_range.is_some() {
                doc.edit_composing(range.start, range.end, new_text);
                doc.end_composition();
            } else if typed {
                // `insert` is the typing path: it coalesces a run of keystrokes
                // into one undo step, where `edit` makes every character its own.
                doc.insert(new_text);
            } else {
                doc.edit(range.start, range.end, new_text);
            }
        }
        // The composition is over: these bytes are the text the user meant.
        self.marked_range = None;
        self.scroll_caret_into_view();
        cx.notify();
    }

    /// The IME's composition step: `new_text` is a *provisional* reading (the
    /// kana of a half-typed Japanese word, the `¨` of a dead-key `ö`) that the
    /// next keystroke will replace outright, and only the last one is the text
    /// the user meant. It goes into the document like any other edit — twig has
    /// no notion of provisional bytes — and `marked_range` is what remembers
    /// which bytes are still up for revision, so the renderer can underline them
    /// and the next call knows what to overwrite.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.doc.is_none() {
            return;
        }
        // gpui hands `range_utf16` straight from NSTextInputClient's
        // `setMarkedText:selectedRange:replacementRange:` — see
        // `marked_replace_range` for what its basis is and why.
        let range = {
            let selection = self.selection_range();
            let marked = self.marked_range.clone();
            let source = &self.doc.as_ref().unwrap().source;
            marked_replace_range(source, range_utf16, marked, selection)
        };

        self.doc
            .as_mut()
            .unwrap()
            .edit_composing(range.start, range.end, new_text);

        // An empty composition is the IME withdrawing it (⎋ out of a candidate
        // window): the text is gone, so there's nothing left to mark.
        self.marked_range = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        if self.marked_range.is_none() {
            // Withdrawn, so the run is over — and it is still one undo step,
            // which now undoes back to before the composition started.
            self.doc.as_mut().unwrap().end_composition();
        }

        // The IME's caret *within* the composition — which syllable a candidate
        // window is offering to replace. Relative to the text just inserted, not
        // to the document. Without this the caret sits at the end of the
        // composition instead of where the IME put it.
        if let (Some(sel), Some(marked)) = (new_selected, self.marked_range.clone()) {
            let base = self.offset_to_utf16(marked.start);
            let want = self.range_from_utf16(&(base + sel.start..base + sel.end));
            self.doc.as_mut().unwrap().place_caret(want.start, false);
        }

        self.scroll_caret_into_view();
        cx.notify();
    }

    /// Where the IME should park its candidate window: the rect of `range_utf16`
    /// in *window* coordinates, which is what gpui's macOS shim wants —
    /// `first_rect_for_character_range` adds the window's frame origin and flips
    /// y for AppKit itself, so anything screen-space here would be wrong twice.
    /// `element_bounds` is already absolute, so adding the row's offset to its
    /// origin is the whole conversion.
    ///
    /// Returning `None` (as this used to, unconditionally) parks the candidate
    /// window at the screen's bottom-left corner.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.last_rows.is_empty() {
            return None; // nothing painted yet; no honest answer to give
        }
        let start = self.offset_from_utf16(range_utf16.start);
        // Empty ranges are the common case, not an edge one: gpui's
        // `compute_ime_candidate_bounds` probes with `caret..caret` and compares
        // the y it gets back to find the line the composition sits on.
        let (r, gi) = locate_caret(&self.last_rows, start);
        let x = self.last_rows[r].x_at(gi) + self.last_row_x.get(r).copied().unwrap_or(px(0.0));
        // The row's top and height from the cumulative tops (variable per row),
        // falling back to the body line height if they're somehow absent.
        let tops = &self.last_row_tops;
        let (y, h) = match (tops.get(r), tops.get(r + 1)) {
            (Some(&t), Some(&b)) => (t, b - t),
            _ => (self.last_line_height * (r as f32), self.last_line_height),
        };
        Some(Bounds::new(
            element_bounds.origin + point(x, y),
            size(px(2.0), h),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

/// Shapes lines, and remembers what it shaped.
///
/// A [`ShapedLine`] is a pure function of its text, its runs, and the font. It
/// holds no source offsets — it doesn't know where in the document its text came
/// from. That is the whole reason this can survive typing.
///
/// An edit shifts every source offset after it, so a cache of *rows* misses on
/// every paragraph below the caret even though their text is untouched; that's
/// what makes typing at the top of a file as expensive as opening it. But the
/// shapes of those paragraphs are bit-for-bit identical, and shaping is the
/// expensive part — 37 ms of a 68 ms paint on a 1 MB document. So the offsets
/// come free from the rebuilt map, and only the shapes are remembered, keyed by
/// what they are actually made of.
///
/// Entries live one paint. `prev` is the last paint's; a shape that gets used
/// moves across into `fresh`, and whatever is still in `prev` at the end was not
/// used and is dropped. So the cache tracks the document as it stands rather
/// than growing with everything it has ever been.
struct Shaper<'w> {
    window: &'w mut Window,
    /// The families and role colors a run is styled with — body/mono family and
    /// the palette, picked per run from each glyph's [`Role`] (see [`build_runs`]).
    styler: RunStyle,
    /// The body font size. A line's size is this, unless its glyphs carry a
    /// heading [`Role`], in which case it scales up by [`Self::heading_scale`] —
    /// see [`Self::line_size`].
    body_size: Pixels,
    /// The per-level heading size multipliers from the theme.
    heading_scale: [f32; 6],
    /// A row's height as a multiple of its font size (the body line height over
    /// the body size). Applied per row, so a heading's taller font gets a
    /// proportionally taller row rather than being crammed onto the body grid.
    line_ratio: f32,
    prev: HashMap<u64, Rc<ShapedLine>>,
    fresh: HashMap<u64, Rc<ShapedLine>>,
    /// Where a logical line wraps, keyed by its content *and the width it was
    /// wrapped at*.
    ///
    /// Finding the breaks means shaping the whole line, but that shape is then
    /// thrown away — the rows are built from the shorter lines it breaks into.
    /// Retaining it alongside them cost 150 MB on a 1 MB document, for a shape
    /// nothing paints. The breaks are a handful of integers and answer the same
    /// question, so a line whose text hasn't changed is never measured twice.
    breaks: HashMap<(u64, u32), Rc<Vec<usize>>>,
    prev_breaks: HashMap<(u64, u32), Rc<Vec<usize>>>,
}

impl Shaper<'_> {
    /// The pixel size a line of these glyphs is shaped at: the body size, scaled
    /// up if they are a heading. Taken as the largest role on the line so a
    /// stray body glyph can't shrink a heading; in practice a heading's glyphs
    /// all carry the same level, and everything else is body.
    fn line_size(&self, glyphs: &[Glyph]) -> Pixels {
        let scale = glyphs
            .iter()
            .map(|g| match g.style.role {
                Role::Heading(l) => heading_scale(l, &self.heading_scale),
                _ => 1.0,
            })
            .fold(1.0f32, f32::max);
        self.body_size * scale
    }

    /// The height of a row of these glyphs — its font size stretched by the
    /// line-height ratio, so a heading's row is taller than a body row.
    fn row_height(&self, glyphs: &[Glyph]) -> Pixels {
        self.line_size(glyphs) * self.line_ratio
    }

    /// Shape `glyphs`, reusing an identical shape from the last paint if there
    /// is one.
    fn shape(&mut self, glyphs: &[Glyph], marked: Option<&Range<usize>>) -> Rc<ShapedLine> {
        let key = shape_key(glyphs, marked);
        self.shape_keyed(key, glyphs, marked)
    }

    /// [`Shaper::shape`] for a caller that has already hashed these glyphs —
    /// the wrap path needs the same key to look up where the line breaks, and
    /// hashing a megabyte of text twice a keystroke is its own bottleneck.
    fn shape_keyed(
        &mut self,
        key: u64,
        glyphs: &[Glyph],
        marked: Option<&Range<usize>>,
    ) -> Rc<ShapedLine> {
        // A hash names the entry; the text confirms it. That pairing is what
        // lets `shape_key` be a fast, weak hash rather than a strong one: a
        // collision costs a re-shape here, where unchecked it would render one
        // paragraph's glyphs in another's place. Verifying is a comparison
        // against the shape's own text — no allocation, and cheap either way.
        if let Some(line) = self.prev.remove(&key).filter(|l| text_is(l, glyphs)) {
            self.fresh.insert(key, line.clone());
            return line;
        }
        if let Some(line) = self.fresh.get(&key).filter(|l| text_is(l, glyphs)) {
            return line.clone();
        }
        let text: String = glyphs.iter().map(|g| g.ch).collect();
        let runs = build_runs(glyphs, &self.styler, marked);
        // The whole line shapes at one size — a run carries a family, not a size
        // — so a heading's larger glyphs are a per-line decision keyed by their
        // role (already folded into `key` via `style_bits`).
        let size = self.line_size(glyphs);
        let line = Rc::new(
            self.window
                .text_system()
                .shape_line(text.into(), size, &runs, None),
        );
        self.fresh.insert(key, line.clone());
        line
    }

    /// The empty line, for a row with nothing on it.
    fn empty(&mut self) -> Rc<ShapedLine> {
        self.shape(&[], None)
    }

    /// Stop retaining a shape — for one that was only ever measured through.
    fn forget(&mut self, key: u64) {
        self.fresh.remove(&key);
    }

    /// The breaks remembered for this line at this width, if it has been
    /// measured before.
    fn cached_breaks(&mut self, key: (u64, u32)) -> Option<Rc<Vec<usize>>> {
        if let Some(b) = self.prev_breaks.remove(&key) {
            self.breaks.insert(key, b.clone());
            return Some(b);
        }
        self.breaks.get(&key).cloned()
    }
}

/// Whether `line` was shaped from exactly these glyphs' characters — the check
/// that makes a 64-bit key exact in practice rather than merely unlikely to be
/// wrong.
fn text_is(line: &ShapedLine, glyphs: &[Glyph]) -> bool {
    line.text.chars().eq(glyphs.iter().map(|g| g.ch))
}

/// Everything a shape is made of, as one number: the characters, the styling
/// that picks each run's font and colour, and whether the IME is underlining it.
/// Deliberately *not* the source offsets — those are what shift under an edit,
/// and the shape doesn't depend on them.
///
/// This runs over every glyph on screen, twice a keystroke, so it is written to
/// be cheap rather than to be a good hash: everything a glyph contributes packs
/// into one `u64`, and each one costs a rotate, an xor, and a multiply. The
/// standard hasher is SipHash, which is built to resist an adversary choosing
/// the keys — there is no adversary here, and paying for that over a megabyte of
/// text cost more than the shaping it was saving.
fn shape_key(glyphs: &[Glyph], marked: Option<&Range<usize>>) -> u64 {
    // FxHash's multiplier, and its rotate-xor-multiply step.
    const K: u64 = 0x51_7c_c1_b7_27_22_0a_95;
    let mut h: u64 = glyphs.len() as u64;
    for g in glyphs {
        let preedit = marked.is_some_and(|m| m.contains(&g.src));
        let packed = (g.ch as u64)
            | ((style_bits(g.style) as u64) << 32)
            | ((preedit as u64) << 48);
        h = (h.rotate_left(5) ^ packed).wrapping_mul(K);
    }
    h
}

/// A style packed into the bits that reach the font: colour and the emphasis
/// flags. `Style` is `Eq` but not `Hash`, and this is the part that matters.
fn style_bits(s: CoreStyle) -> u16 {
    // The role reaches the shape more than one way — it picks the run's family
    // (mono for code), color, and the line's size (larger for a heading) — so two
    // otherwise-equal glyphs that differ only in role must not share a shape.
    let role = match s.role {
        Role::Body => 0u16,
        Role::Code => 1,
        Role::Link => 2,
        Role::Mark => 3,
        Role::ListMarker => 4,
        Role::QuoteGutter => 5,
        Role::Image => 6,
        Role::Rule => 7,
        Role::Heading(l) => 8 + l.min(7) as u16, // 8..=15, four bits
    };
    (s.bold as u16)
        | ((s.italic as u16) << 1)
        | ((s.underline as u16) << 2)
        | ((s.strikethrough as u16) << 3)
        | (role << 4)
}

/// One shaped run of text within a row, placed at a known x from the row's left.
///
/// Prose has exactly one of these per row, at x = 0. A table's grid row has one
/// per column, each placed at its column's own x — which is the whole reason a
/// row is a list of these rather than a single shaped line: a proportional font
/// can't be padded to a column with spaces.
struct RowSegment {
    x: Pixels,
    /// Shared with the shape cache — the same shape may be on several rows, and
    /// on the same row across paints.
    shaped: Rc<ShapedLine>,
    /// The byte offset within *this segment's* text of each of its characters,
    /// plus a final entry for the text's end (`chars + 1` entries).
    char_byte: Vec<usize>,
    /// Where this segment's first character sits in the row's flat `char_srcs`.
    first: usize,
    /// The span of x this segment answers for a click in — its whole column in a
    /// table, which is wider than its text: a cell's padding and the border
    /// beside it belong to that cell, so clicking them lands the caret in it
    /// rather than in whichever cell's text happens to be nearer. (A short cell
    /// next to a wide column is exactly the case that gets that wrong.) For prose
    /// it's just the text, which is moot — there's only ever one segment.
    field: (Pixels, Pixels),
}

/// One painted row: its shaped segments plus the mapping the caret/selection/
/// mouse code rides on. `char_srcs[i]` is the source byte offset the i-th
/// character came from, flat across the row's segments in document order (a
/// table row's cells run left to right, and so do their offsets). `end_src` is
/// the source offset the caret lands on past the last character.
///
/// For a source line this is trivial (one segment; each char maps to its own
/// source byte); for a WYSIWYG row the glyphs carry `Glyph::src`, so a hidden
/// delimiter simply has no character here and the caret steps over it.
struct RowLayout {
    segments: Vec<RowSegment>,
    char_srcs: Vec<usize>,
    end_src: usize,
    /// This row's own height. A body row is the theme's line height; a heading
    /// row is taller, since its font is bigger. Rows are stacked by summing
    /// these rather than by a single line height (see [`row_tops`]).
    height: Pixels,
}

impl RowLayout {
    /// A row of ordinary prose: one segment, flush left.
    fn prose(
        shaped: Rc<ShapedLine>,
        char_srcs: Vec<usize>,
        char_byte: Vec<usize>,
        end_src: usize,
        height: Pixels,
    ) -> Self {
        let field = (px(0.0), shaped.width);
        RowLayout {
            segments: vec![RowSegment { x: px(0.0), shaped, char_byte, first: 0, field }],
            char_srcs,
            end_src,
            height,
        }
    }

    /// The segment holding flat character index `gi` — the last one that opens at
    /// or before it, so `gi == char_srcs.len()` ("past the last character") lands
    /// on the final segment's end.
    fn segment_of(&self, gi: usize) -> Option<usize> {
        (!self.segments.is_empty()).then(|| {
            self.segments
                .iter()
                .rposition(|s| s.first <= gi)
                .unwrap_or(0)
        })
    }

    /// The x the caret draws at for flat character index `gi`.
    fn x_at(&self, gi: usize) -> Pixels {
        match self.segment_of(gi) {
            Some(si) => self.x_in(si, gi),
            None => px(0.0),
        }
    }

    /// The x of flat character index `gi` *within segment `si`* — including the
    /// spot just past that segment's last character, which `x_at` can't name: an
    /// index at a segment boundary belongs to the next segment, so asking `x_at`
    /// where a cell's text *ends* answers with where the next cell's text
    /// begins, a whole border and two gutters away. Selection highlighting needs
    /// the end of the run it actually measured.
    fn x_in(&self, si: usize, gi: usize) -> Pixels {
        let seg = &self.segments[si];
        let local = gi.saturating_sub(seg.first).min(seg.char_byte.len() - 1);
        seg.x + seg.shaped.x_for_index(seg.char_byte[local])
    }

    /// The flat character index nearest x — where a click lands. x is relative to
    /// the row's left edge.
    fn index_for_x(&self, x: Pixels) -> usize {
        let Some(si) = self.segment_nearest_x(x) else {
            return 0;
        };
        let seg = &self.segments[si];
        let byte = seg.shaped.closest_index_for_x(x - seg.x);
        let local = seg
            .char_byte
            .iter()
            .position(|&b| b == byte)
            .unwrap_or(seg.char_byte.len() - 1);
        let gi = seg.first + local;
        // "Past the last character" belongs to the *last* segment only. Anywhere
        // else that spot is already the next segment's first character — a
        // different cell — so a click in a cell's right gutter would jump the
        // caret into its neighbour. A table cell needs no such spot anyway: its
        // trailing gutter space carries the cell's end stop.
        match self.segments.get(si + 1) {
            Some(next) => gi.min(next.first - 1),
            None => gi,
        }
    }

    /// The segment whose field `x` falls in, or the nearest one — measured to the
    /// field, not the text, so a click in a cell's padding stays in that cell.
    /// Falling back to the nearest keeps a click past the end of a row (or in a
    /// ragged row's missing cell) landing somewhere rather than nowhere.
    fn segment_nearest_x(&self, x: Pixels) -> Option<usize> {
        self.segments
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let d = |s: &RowSegment| {
                    let (l, r) = s.field;
                    if x < l {
                        l - x
                    } else if x > r {
                        x - r
                    } else {
                        px(0.0)
                    }
                };
                d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    }

    /// The source offset at flat character index `gi`, or the row's end past the
    /// last character.
    fn src_at_index(&self, gi: usize) -> usize {
        self.char_srcs.get(gi).copied().unwrap_or(self.end_src)
    }
}

/// The top y of every row, relative to the text origin, plus a final entry for
/// the bottom of the last row — a running sum of the rows' own heights. This is
/// what replaces "row index × one line height" now that a heading's row is
/// taller than a body row: `tops[r]` is where row `r` starts, `tops[r + 1]` is
/// where it ends. Has `rows.len() + 1` entries (`[0]` for an empty document).
fn row_tops(rows: &[RowLayout]) -> Vec<Pixels> {
    let mut tops = Vec::with_capacity(rows.len() + 1);
    let mut y = px(0.0);
    tops.push(y);
    for row in rows {
        y += row.height;
        tops.push(y);
    }
    tops
}

/// The row a y (relative to the text origin) falls in — the inverse of
/// [`row_tops`], for turning a click's pixel into a row. Clamped to a real row:
/// a y above the first lands on row 0, one past the last on the final row.
fn row_at_y(tops: &[Pixels], y: Pixels) -> usize {
    // `tops` ascends, so the row is the last top at or below `y`. `partition_point`
    // counts the tops `<= y`; one before that is the row index. The final entry is
    // the bottom edge, not a row, so the answer clamps to `rows - 1 == len - 2`.
    tops.partition_point(|&t| t <= y)
        .saturating_sub(1)
        .min(tops.len().saturating_sub(2))
}

/// The document body element: builds a [`RowLayout`] per visual row of the
/// active view, paints them with the caret and selection, and installs the
/// input handler.
struct TextElement {
    editor: Entity<Editor>,
}

struct Prepaint {
    /// Shared with the editor's `last_rows` — the same shaped lines the mouse
    /// hit-tests against, not a copy of them.
    rows: Rc<Vec<RowLayout>>,
    /// The cumulative row tops (see [`row_tops`]) — where each row is painted,
    /// and what the editor keeps for the next frame's mouse/scroll math.
    tops: Rc<Vec<Pixels>>,
    /// The horizontal delta added to each row's text — a code block's
    /// indent-minus-scroll, zero for prose. Kept for the editor's mouse math.
    row_x: Rc<Vec<Pixels>>,
    /// A representative body line height, stored back for page up/down.
    line_height: Pixels,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
    /// A table's header/stripe fills, painted under the selection, and its rules,
    /// painted over it.
    table_fills: Vec<PaintQuad>,
    table_borders: Vec<PaintQuad>,
    /// A code block's border-and-tint box, painted under the selection and text.
    code_fills: Vec<PaintQuad>,
    /// Each code block's rows and box rect — the rect clips its (scrolled) text
    /// during paint so nothing spills past the box.
    code_boxes: Vec<(Range<usize>, Bounds<Pixels>)>,
    /// Each code block's language label: the shaped chip, its text origin, and
    /// the chip's background rect. Painted over the box border.
    code_labels: Vec<(Rc<ShapedLine>, Point<Pixels>, Bounds<Pixels>)>,
    /// The label chip's line height, for painting the shaped label.
    code_label_h: Pixels,
    /// Each block image's on-screen box and the raster to paint into it — the
    /// rects resolved from the row tops this paint. Drawn over the text pass.
    images: Vec<(Bounds<Pixels>, Arc<RenderImage>)>,
}

/// Everything the painted rows are a function of. Two paints with equal keys
/// would shape identical rows, so the second one doesn't.
///
/// The width is carried as raw bits because `f32` isn't `Eq` — and a width that
/// differs by a fraction of a pixel really does re-wrap, so rounding it would be
/// a bug rather than a tolerance.
#[derive(PartialEq, Eq, Clone)]
struct LayoutKey {
    /// `Doc::revision` — every edit, undo, redo, and reload; no motion, no save.
    revision: u64,
    width: u32,
    source_view: bool,
    /// The IME preedit underlines glyphs, so it changes what gets shaped.
    marked: Option<Range<usize>>,
}

/// One unit of the document as prepaint gathers it: a line of text to be pixel-
/// wrapped, or a table to be laid out as a grid.
///
/// A `Line`'s `code` is the index of the code block it belongs to (in
/// [`VisualMap::code_blocks`] order), or `None` for ordinary prose. A code line
/// isn't wrapped — it scrolls horizontally inside its box instead — and the
/// output rows it produces are gathered into that block's on-screen span so the
/// element can draw one border-and-tint box around the whole run.
enum Logical {
    Line { glyphs: Vec<Glyph>, end_src: usize, code: Option<usize>, decoration: bool },
    Table(TableInfo),
    /// A block-level image, whose placeholder row (the `🖼 alt` picture core
    /// draws) is skipped the way a table's box rows are — the GUI paints the real
    /// raster instead. `glyphs`/`end_src` are copied from that placeholder row so
    /// the reserved row still carries the image's caret stops (a home in front of
    /// it and one just past it); `info` carries the destination to load and the
    /// alt text to fall back to.
    Image { info: ImageInfo, glyphs: Vec<Glyph>, end_src: usize },
}

/// Gather the document into the units prepaint lays out: one line per source line
/// (Source) or per map row (WYSIWYG), and a table wherever the map's *picture* of
/// one begins — whose rows are skipped, because the GUI draws its own.
///
/// Skipping is the whole job. `leaf-core` spells a table with box glyphs on
/// monospace columns, which is right in a terminal and shears in a proportional
/// font; letting those rows through would paint the old picture underneath the
/// new grid.
fn gather_logical(doc: &Doc) -> Vec<Logical> {
    let mut lines = Vec::new();
    match doc.view {
        View::Source => {
            let mut start = 0usize;
            for line in doc.source.split('\n') {
                let glyphs: Vec<Glyph> = line
                    .char_indices()
                    .map(|(i, ch)| Glyph {
                        ch,
                        style: CoreStyle::default(),
                        src: start + i,
                        // Raw source: every char is real text.
                        stop: true,
                    })
                    .collect();
                lines.push(Logical::Line {
                    glyphs,
                    end_src: start + line.len(),
                    code: None,
                    decoration: false,
                });
                start += line.len() + 1;
            }
        }
        View::Wysiwyg => {
            // `tables`, `code_blocks`, and `images` are all in row order, so one
            // cursor apiece walks them alongside the rows rather than re-scanning.
            let mut next_table = doc.vmap.tables.iter().peekable();
            let mut next_image = doc.vmap.images.iter().peekable();
            let mut code = doc.vmap.code_blocks.iter().enumerate().peekable();
            let mut r = 0usize;
            while r < doc.vmap.rows.len() {
                // An image never sits inside a table or code block, so the image
                // cursor only needs to stay abreast of `r`.
                while next_image.peek().is_some_and(|im| im.rows_span.end <= r) {
                    next_image.next();
                }
                match next_table.peek().filter(|t| t.rows_span.start == r) {
                    Some(t) => {
                        lines.push(Logical::Table((*t).clone()));
                        r = t.rows_span.end;
                        next_table.next();
                    }
                    _ if next_image.peek().is_some_and(|im| im.rows_span.start == r) => {
                        // Skip the placeholder picture like a table's box rows; the
                        // reserved row keeps the caret stops the vrow carries.
                        let im = next_image.next().unwrap();
                        let vrow = &doc.vmap.rows[r];
                        lines.push(Logical::Image {
                            info: im.clone(),
                            glyphs: vrow.glyphs.clone(),
                            end_src: vrow.end_src,
                        });
                        r = im.rows_span.end;
                    }
                    None => {
                        // Which code block, if any, this row falls inside — the
                        // cursor advances past a block once its rows are behind us.
                        while code.peek().is_some_and(|(_, c)| c.rows_span.end <= r) {
                            code.next();
                        }
                        let in_code = code
                            .peek()
                            .filter(|(_, c)| c.rows_span.contains(&r))
                            .map(|(i, _)| *i);
                        let vrow = &doc.vmap.rows[r];
                        lines.push(Logical::Line {
                            glyphs: vrow.glyphs.clone(),
                            end_src: vrow.end_src,
                            code: in_code,
                            // Tables and images are already skipped above, so the
                            // only decoration rows reaching here are the blank
                            // block-gap separators — laid out short below.
                            decoration: vrow.decoration,
                        });
                        r += 1;
                    }
                }
            }
        }
    }
    lines
}

/// Merge a row's per-glyph styles into `TextRun`s (adjacent glyphs of equal
/// style become one run), then map each through `to_gpui`/`text_run`.
///
/// `marked` is the IME's live composition, which underlines the glyphs whose
/// *source* byte falls inside it — so it segments runs alongside the style, and
/// rides the glyphs' `src` exactly like everything else here (a preedit in
/// WYSIWYG is underlined across the visible text, hidden delimiters and all).
fn build_runs(glyphs: &[Glyph], styler: &RunStyle, marked: Option<&Range<usize>>) -> Vec<gpui::TextRun> {
    let mut segs: Vec<(usize, CoreStyle, bool)> = Vec::new();
    for g in glyphs {
        let bytes = g.ch.len_utf8();
        let preedit = marked.is_some_and(|m| m.contains(&g.src));
        if let Some(last) = segs.last_mut()
            && last.1 == g.style
            && last.2 == preedit
        {
            last.0 += bytes;
            continue;
        }
        segs.push((bytes, g.style, preedit));
    }
    segs.into_iter()
        .map(|(len, st, preedit)| {
            let mut run = text_run(len, st, styler);
            if preedit {
                // `color: None` inherits the run's own text color, so the
                // underline follows a preedit composed inside styled text.
                run.underline = Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: None,
                    wavy: false,
                });
            }
            run
        })
        .collect()
}

/// Pixel-wrap one logical line (a paragraph, or a source line) into visual rows,
/// pushing a [`RowLayout`] per row. This is the *true* proportional wrap that
/// replaces leaf-core's monospace character-count estimate: it shapes the whole
/// line once to measure real glyph advances, then greedily breaks it at word
/// boundaries wherever the measured width exceeds `wrap_px`. A line that doesn't
/// need to wrap reuses its single shaped line as-is (no re-shaping).
fn wrap_logical(
    shaper: &mut Shaper,
    glyphs: &[Glyph],
    logical_end_src: usize,
    wrap_px: f32,
    marked: Option<&Range<usize>>,
    height_override: Option<Pixels>,
    out: &mut Vec<RowLayout>,
) {
    // Every visual row of one logical line shares its role, so its height too:
    // a heading wraps into taller rows, a paragraph into body-height ones. A
    // block-gap separator overrides this with a shrunk gap height so a paragraph
    // boundary reads as spacing, not a blank line.
    let height = height_override.unwrap_or_else(|| shaper.row_height(glyphs));
    if glyphs.is_empty() {
        let shaped = shaper.empty();
        out.push(RowLayout::prose(shaped, Vec::new(), vec![0], logical_end_src, height));
        return;
    }

    let char_byte = char_bytes(glyphs);
    let n = glyphs.len();
    let hash = shape_key(glyphs, marked);
    let key = (hash, wrap_px.to_bits());

    // Where this line breaks. Known already unless its text or the width moved —
    // which is the difference between a keystroke re-measuring the paragraph it
    // landed in and re-measuring the whole document.
    let starts = match shaper.cached_breaks(key) {
        Some(starts) => starts,
        None => {
            // Shape the whole line once, purely to measure where the breaks
            // fall.
            let full = shaper.shape_keyed(hash, glyphs, marked);
            let x = |byte: usize| f32::from(full.x_for_index(byte));

            // Greedy word wrap: walk maximal non-space runs, breaking before a
            // word when the line up to that word's end would overflow.
            let mut starts = vec![0usize]; // glyph index each visual row starts at
            let mut line_start = 0usize;
            let mut j = 0usize;
            while j < n {
                if glyphs[j].ch == ' ' {
                    j += 1;
                    continue;
                }
                let word_start = j;
                while j < n && glyphs[j].ch != ' ' {
                    j += 1;
                }
                if x(char_byte[j]) - x(char_byte[line_start]) > wrap_px && word_start > line_start {
                    starts.push(word_start);
                    line_start = word_start;
                }
            }
            drop(full);
            if starts.len() > 1 {
                // It wraps, so the rows are built from the shorter lines below
                // and nothing will ever paint the whole-line shape. Keeping it
                // would double the memory the cache holds for every paragraph on
                // screen; the breaks say all we needed it for.
                shaper.forget(hash);
            }
            let starts = Rc::new(starts);
            shaper.breaks.insert(key, starts.clone());
            starts
        }
    };

    // The common case — the line fits — is its own single row, and the shape
    // measured above is the one it paints.
    if starts.len() == 1 {
        out.push(RowLayout::prose(
            shaper.shape_keyed(hash, glyphs, marked),
            glyphs.iter().map(|g| g.src).collect(),
            char_byte,
            logical_end_src,
            height,
        ));
        return;
    }

    for k in 0..starts.len() {
        let gs = starts[k];
        let ge = starts.get(k + 1).copied().unwrap_or(n);
        let sub = &glyphs[gs..ge];
        let scb = char_bytes(sub);
        let shaped = shaper.shape(sub, marked);
        // The offset the caret lands on past this row: the block's end on the
        // last row, else the start of the next row's first glyph.
        let end_src = if ge == n {
            logical_end_src
        } else {
            let last = &glyphs[ge - 1];
            last.src + last.ch.len_utf8()
        };
        out.push(RowLayout::prose(
            shaped,
            sub.iter().map(|g| g.src).collect(),
            scb,
            end_src,
            height,
        ));
    }
}

// ── tables ───────────────────────────────────────────────────────────────────
//
// `leaf-core` draws a table with box glyphs, which is exactly right in a
// terminal and unfixable here: in a proportional font the `│`s of two rows land
// at different x and the grid shears. So the GUI reads the table's *structure*
// (`VisualMap::tables`), skips the rows that picture occupies, and lays the grid
// out in pixels — measuring each cell, sizing each column to its widest, and
// painting the borders as quads.
//
// The cells stay ordinary text: one `RowSegment` each, so the caret, selection,
// and IME reach into them by the same path as any paragraph.

/// The border thickness, and the breathing room either side of a cell's text.
const BORDER: f32 = 1.0;
const CELL_PAD_X: f32 = 8.0;

/// A fenced code block's box: the border thickness, the horizontal breathing
/// room between the border and the code (the text is indented this far and its
/// scroll keeps it clear of both edges), and the vertical padding the box grows
/// past its rows into the blank separator lines above and below. The rounded
/// corner radius softens the box the way an inline code pill is soft.
const CODE_BORDER: f32 = 1.0;
const CODE_PAD_X: f32 = 8.0;
const CODE_PAD_Y: f32 = 4.0;
const CODE_RADIUS: f32 = 4.0;
/// How far the caret is kept from the right edge of a code block as it scrolls —
/// a little runway so the next character typed is already visible.
const CODE_CARET_MARGIN: f32 = 24.0;
/// The language label's font size relative to the body — a small chip on the box.
const CODE_LABEL_SCALE: f32 = 0.8;

/// The narrowest a column may be squeezed — below this a column stops carrying
/// text and shreds it a letter per line, which is worse than running wide. The
/// pixel echo of `leaf-core`'s `MIN_COL_WIDTH`.
const MIN_COL_PX: f32 = 24.0;

/// Where a table's chrome goes: everything needed to paint its borders and fills
/// once the rows around it have been placed and their y is known.
struct TableGeom {
    /// The rows of the flat row list this table's grid lines occupy.
    rows: Range<usize>,
    /// The x of every column boundary, `cols + 1` of them: `bounds[0]` is the
    /// table's left edge and `bounds[cols]` its right, each the *centre* of the
    /// border drawn there.
    bounds: Vec<f32>,
    /// Per logical table row: the grid lines it spans (a wrapped cell makes a row
    /// taller than one), and whether it's a header.
    bands: Vec<(Range<usize>, bool)>,
}

/// Where a code block's box goes: the flat output rows its lines occupy, filled
/// once their y is known. Its horizontal scroll is not stored here — it's
/// recomputed each paint from the caret, since the caret moves without the rows
/// re-shaping (so it can't ride the row cache).
struct CodeGeom {
    rows: Range<usize>,
    /// The block's language, painted as a small label on the box — `None` for a
    /// bare fence or indented block. Carried here so it rides the row cache with
    /// the geometry it labels.
    lang: Option<String>,
}

/// Where a block-level image's raster is painted: the output row it reserves and
/// the decoded frame, plus the box size (an aspect-preserving fit within the
/// editor width). Like a [`CodeGeom`] it rides the row cache; the on-screen rect
/// is recomputed each paint from the row tops, since it depends on `bounds` —
/// which the row-cache key doesn't cover.
#[derive(Clone)]
struct ImageGeom {
    row: usize,
    image: Arc<RenderImage>,
    /// The painted box size, in logical pixels.
    size: Size<Pixels>,
}

/// The tallest a block image is drawn, in logical pixels — a very tall image is
/// scaled down to this so a single picture can't push the whole document
/// off-screen.
const IMAGE_MAX_H: f32 = 480.0;
/// Vertical breathing room above and below an image's box.
const IMAGE_PAD_Y: f32 = 6.0;

/// The window's light/dark appearance as a [`ColorScheme`], for picking a
/// `<picture>`'s `prefers-color-scheme` source (see [`ImageInfo::resolve`]). GPUI
/// already tracks the system appearance and repaints on a change, so a theme
/// switch re-picks the source on the next frame for free. The desktop app and
/// the mobile host (leaf-ios) share this one upstream gpui `WindowAppearance`,
/// so no platform split is needed; the vibrant variants fold to their base tone.
fn window_color_scheme(window: &Window) -> ColorScheme {
    use gpui::WindowAppearance;
    match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ColorScheme::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => ColorScheme::Light,
    }
}

/// Resolve an image destination to a readable local file path, or `None` when it
/// isn't one this synchronous loader can handle: a remote URL (`http(s):`), a
/// `data:` URI, a protocol-relative `//host/…`, or a relative path with no
/// document directory to resolve it against (an untitled buffer). A relative path
/// is joined to the document's directory; an absolute path is taken as-is. Those
/// unsupported cases fall back to the `🖼 alt` text placeholder, painted the way
/// core's default rendering already spells them.
fn resolve_image_path(dest: &str, doc_dir: Option<&Path>) -> Option<PathBuf> {
    let dest = dest.trim();
    if dest.is_empty() {
        return None;
    }
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || dest.starts_with("//")
    {
        return None;
    }
    // A `file:` URL is just a path wearing a scheme.
    let raw = dest.strip_prefix("file://").unwrap_or(dest);
    let path = Path::new(raw);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        doc_dir.map(|d| d.join(path))
    }
}

/// Decode an image file to a gpui [`RenderImage`], or `None` on any failure (a
/// missing/unreadable file, an unsupported or corrupt format, or an SVG — which
/// the `image` crate doesn't rasterize). gpui paints **BGRA** while `image`
/// decodes **RGBA**, so every pixel's R and B bytes are swapped before the frame
/// is handed over — the same swap gpui itself does everywhere it builds a
/// `RenderImage`.
fn load_image_file(path: &Path) -> Option<Arc<RenderImage>> {
    let bytes = std::fs::read(path).ok()?;
    let format = image::guess_format(&bytes).ok()?;
    let mut rgba = image::load_from_memory_with_format(&bytes, format)
        .ok()?
        .into_rgba8();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2); // RGBA → BGRA
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

/// Fit an image's intrinsic pixel size into a box no wider than `avail` and no
/// taller than [`IMAGE_MAX_H`], preserving aspect ratio and never upscaling past
/// the intrinsic size.
fn image_box_size(intrinsic: Size<DevicePixels>, avail: f32) -> Size<Pixels> {
    let iw = intrinsic.width.0.max(1) as f32;
    let ih = intrinsic.height.0.max(1) as f32;
    // Never wider than the editor, never upscaled past the source.
    let mut w = iw.min(avail.max(1.0));
    let mut h = w * ih / iw;
    if h > IMAGE_MAX_H {
        h = IMAGE_MAX_H;
        w = h * iw / ih;
    }
    size(px(w), px(h))
}

/// Shrink `widths` until the grid fits `avail`, taking from the widest column
/// each time so the loss is shared rather than falling on whichever column is
/// last. No column goes below [`MIN_COL_PX`]; a table with more columns than the
/// surface has room for still overflows, which is the honest outcome — there's
/// nothing left to give. The pixel counterpart of `leaf-core`'s `fit_widths`.
fn fit_widths_px(widths: &mut [f32], avail: f32) {
    // Chrome: every column carries a border and a gutter either side, and one
    // more border closes the grid.
    let chrome = widths.len() as f32 * (BORDER + 2.0 * CELL_PAD_X) + BORDER;
    let budget = (avail - chrome).max(0.0);
    // A whole pixel at a time: the widths are a few hundred at most, and stepping
    // keeps the shrink hitting the widest column rather than scaling every column
    // by a ratio (which would squeeze a narrow column that was already fine).
    while widths.iter().sum::<f32>() > budget {
        let Some(w) = widths
            .iter_mut()
            .filter(|w| **w > MIN_COL_PX)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        else {
            return;
        };
        // Clamped, not just decremented: real widths are fractional, so a column
        // at 24.5 would step straight through a 24.0 floor.
        *w = (*w - 1.0).max(MIN_COL_PX);
    }
}

/// Word-wrap `glyphs` to `width` pixels, hard-breaking any single word too long
/// to fit.
///
/// Unlike a paragraph — where an overlong word just trails off the end of the
/// line — a table column is a hard boundary: a glyph past it lands on the border
/// or in the next cell. So the width here is a promise, and a word that won't
/// keep it is broken. A break only ever falls between grapheme clusters (on a
/// glyph that opens one), since the caller anchors each line's end stop just past
/// its last glyph — a line cut mid-cluster would put a caret stop inside a
/// character, and the next Backspace would take it apart from the middle.
fn wrap_glyphs_px(
    shaper: &mut Shaper,
    glyphs: &[Glyph],
    width: f32,
) -> Vec<Vec<Glyph>> {
    if glyphs.is_empty() {
        return vec![Vec::new()];
    }
    let char_byte = char_bytes(glyphs);
    let hash = shape_key(glyphs, None);
    let shaped = shaper.shape_keyed(hash, glyphs, None);
    let x = |i: usize| f32::from(shaped.x_for_index(char_byte[i]));

    let n = glyphs.len();
    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    let mut start = 0usize;
    while start < n {
        // The furthest this line can reach.
        let mut end = start;
        while end < n && x(end + 1) - x(start) <= width {
            end += 1;
        }
        // One glyph always goes on, however narrow the column: a line that took
        // nothing would never advance.
        end = end.max(start + 1);
        if end < n {
            // Prefer a word boundary, and drop the space it breaks at — its
            // offset isn't lost, the caller's end stop sits exactly there.
            if let Some(sp) = glyphs[start..end].iter().rposition(|g| g.ch == ' ')
                && sp > 0
            {
                lines.push(glyphs[start..start + sp].to_vec());
                start += sp + 1;
                continue;
            }
            // Breaking mid-word: back up to the start of a grapheme cluster.
            while end > start + 1 && !glyphs[end].stop {
                end -= 1;
            }
        }
        lines.push(glyphs[start..end].to_vec());
        start = end;
    }
    if lines.len() > 1 {
        // Wrapped: the cell is painted from the shorter lines, so the whole-cell
        // shape was only ever a measurement — see `Shaper::breaks`.
        drop(shaped);
        shaper.forget(hash);
    }
    lines
}

/// Lay a table out in pixels and push a [`RowLayout`] per grid line, returning
/// the geometry its chrome is painted from.
///
/// Columns are sized to content — each as wide as its widest cell — and only
/// squeezed if the grid won't fit, at which point cells wrap into what's left.
fn layout_table(
    shaper: &mut Shaper,
    info: &TableInfo,
    avail: f32,
    marked: Option<&Range<usize>>,
    out: &mut Vec<RowLayout>,
) -> Option<TableGeom> {
    let cols = info.grid.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    if cols == 0 || info.grid.is_empty() {
        return None;
    }

    // A table is body text throughout, so every grid line is a body-height row.
    let row_h = shaper.row_height(&[]);

    // The block prefix — a quote's gutter, a list's indent — is drawn on every
    // grid row and the table starts past it, the way the picture does it.
    let prefix = shape_glyphs(shaper, &info.prefix, marked);
    let indent = f32::from(prefix.as_ref().map_or(px(0.0), |p| p.1.width));

    // Every cell renders a trailing gutter space, which carries its end stop —
    // so it's part of what a column has to hold, and part of what a wrapped line
    // has to leave room for.
    let space = measure_glyphs(shaper, &[]);

    // Every column at its widest cell is the wish; `fit_widths_px` is what the
    // surface can actually give. The measurements are kept: a cell that already
    // fits its column needs no second pass to find out where it wraps.
    let mut widths = vec![0f32; cols];
    let measured: Vec<Vec<f32>> = info
        .grid
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| measure_glyphs(shaper, &cell.glyphs))
                .collect()
        })
        .collect();
    for row in &measured {
        for (c, &w) in row.iter().enumerate() {
            widths[c] = widths[c].max(w);
        }
    }
    fit_widths_px(&mut widths, avail - indent);

    // Column boundaries, each the centre of the border drawn there.
    let mut bounds = vec![indent; cols + 1];
    for c in 0..cols {
        bounds[c + 1] = bounds[c] + BORDER + CELL_PAD_X + widths[c] + CELL_PAD_X;
    }

    let rows_start = out.len();
    let mut bands = Vec::new();
    for (ri, row) in info.grid.iter().enumerate() {
        let band_start = out.len();
        // Each cell wraps into its own column, and they run out at their own
        // heights; the row is as tall as the tallest.
        let laid: Vec<Vec<Vec<Glyph>>> = (0..cols)
            .map(|c| match row.cells.get(c) {
                // The common case is a cell that already fits, which is its own
                // single line — no need to shape it again to discover that.
                Some(cell) if measured[ri][c] <= widths[c] => vec![cell.glyphs.clone()],
                // The wrap width leaves room for the gutter space appended
                // below, or the line plus its space would overrun the column.
                Some(cell) => {
                    wrap_glyphs_px(shaper, &cell.glyphs, (widths[c] - space).max(1.0))
                }
                None => vec![Vec::new()],
            })
            .collect();
        let height = laid.iter().map(|l| l.len()).max().unwrap_or(1).max(1);

        for j in 0..height {
            let mut segments = Vec::new();
            let mut char_srcs: Vec<usize> = Vec::new();
            // The prefix opens every grid row, before the grid itself — its
            // glyphs point at the enclosing block, so a click in the gutter lands
            // there rather than in a cell.
            if let Some((glyphs, shaped, char_byte)) = prefix.clone() {
                segments.push(RowSegment {
                    x: px(0.0),
                    shaped,
                    char_byte,
                    first: 0,
                    field: (px(0.0), px(indent)),
                });
                char_srcs.extend(glyphs.iter().map(|g| g.src));
            }
            for c in 0..cols {
                let (Some(cell), Some(line)) = (row.cells.get(c), laid[c].get(j)) else {
                    continue; // a ragged row, or a column that ran dry higher up
                };
                // The offset the caret lands on past this line's text: the cell's
                // end on its last line, else the space the wrap consumed.
                let last = laid[c].len() == j + 1;
                let end = match last {
                    true => cell.end,
                    false => line
                        .last()
                        .map(|g| g.src + g.ch.len_utf8())
                        .unwrap_or(cell.end),
                };
                // A trailing space carries that end stop, so there is always
                // somewhere to put the "after the last character" caret — the
                // same trick `leaf-core` plays with a cell's gutter.
                let mut gs = line.clone();
                gs.push(Glyph {
                    ch: ' ',
                    style: CoreStyle::default(),
                    src: end,
                    stop: true,
                });
                let char_byte = char_bytes(&gs);
                let shaped = shaper.shape(&gs, marked);

                let slack = (widths[c] - f32::from(shaped.width)).max(0.0);
                let lead = match cell.align {
                    Alignment::Right => slack,
                    Alignment::Center => slack / 2.0,
                    _ => 0.0,
                };
                segments.push(RowSegment {
                    x: px(bounds[c] + BORDER + CELL_PAD_X + lead),
                    shaped,
                    char_byte,
                    first: char_srcs.len(),
                    // The cell answers for its whole column, borders included.
                    field: (px(bounds[c]), px(bounds[c + 1])),
                });
                char_srcs.extend(gs.iter().map(|g| g.src));
            }
            // The row ends where its last cell's end stop does.
            let end_src = char_srcs.last().copied().unwrap_or(info.end_src);
            out.push(RowLayout { segments, char_srcs, end_src, height: row_h });
        }
        bands.push((band_start..out.len(), row.head));
    }

    Some(TableGeom {
        rows: rows_start..out.len(),
        bounds,
        bands,
    })
}

/// Build a table's fills and borders, now that the rows around it have been
/// placed and their y is known.
///
/// Returned as (fills, borders) so they can be painted either side of the
/// selection: a header or stripe belongs *under* the selection highlight, and
/// the rules belong *over* it, or a selection spanning a cell boundary would
/// swallow the grid.
fn table_chrome(
    geom: &TableGeom,
    left: Pixels,
    top: Pixels,
    tops: &[Pixels],
    style: &EditorStyle,
) -> (Vec<PaintQuad>, Vec<PaintQuad>) {
    // A grid line's y is read from the cumulative row tops, not a uniform line
    // height — a heading above the table would otherwise misplace the whole grid.
    let y = |row: usize| top + tops[row.min(tops.len() - 1)];
    let x = |v: f32| left + px(v);
    let (x0, x1) = (x(geom.bounds[0]), x(*geom.bounds.last().unwrap()));
    let (y0, y1) = (y(geom.rows.start), y(geom.rows.end));

    // The header's fill, then a tint on every other body row. Striping counts
    // *bands*, not grid lines, so a row with a wrapped cell is one stripe rather
    // than two — the banding follows the table, not the text that overflowed.
    let mut fills = Vec::new();
    let mut body = 0usize;
    for (band, head) in &geom.bands {
        let bg = match head {
            true => Some(style.table_header),
            false => {
                body += 1;
                // The first body row stays clear, so the header is the only
                // filled row at the top rather than one of two.
                (body % 2 == 0).then_some(style.table_stripe)
            }
        };
        if let Some(bg) = bg {
            fills.push(fill(
                Bounds::from_corners(
                    point(x0, y(band.start)),
                    point(x1 + px(BORDER), y(band.end)),
                ),
                bg,
            ));
        }
    }

    let mut borders = Vec::new();
    let mut rule = |a: Point<Pixels>, b: Point<Pixels>| {
        borders.push(fill(Bounds::from_corners(a, b), style.table_border));
    };
    // Verticals: every column boundary, the outer two included.
    for &b in &geom.bounds {
        rule(point(x(b), y0), point(x(b) + px(BORDER), y1));
    }
    // Horizontals: the table's top and bottom, and a rule between bands.
    let mut edges: Vec<usize> = vec![geom.rows.start, geom.rows.end];
    edges.extend(geom.bands.iter().skip(1).map(|(r, _)| r.start));
    for e in edges {
        let ey = y(e).min(y1 - px(BORDER));
        rule(point(x0, ey), point(x1 + px(BORDER), ey + px(BORDER)));
    }
    (fills, borders)
}

/// Shape `glyphs` as one run, or `None` if there are none — the shape a prefix
/// segment is built from. Returns the glyphs alongside, since the caller needs
/// their source offsets to go on carrying the caret.
#[allow(clippy::type_complexity)]
fn shape_glyphs(
    shaper: &mut Shaper,
    glyphs: &[Glyph],
    marked: Option<&Range<usize>>,
) -> Option<(Vec<Glyph>, Rc<ShapedLine>, Vec<usize>)> {
    if glyphs.is_empty() {
        return None;
    }
    Some((
        glyphs.to_vec(),
        shaper.shape(glyphs, marked),
        char_bytes(glyphs),
    ))
}

/// The width `glyphs` shape to, plus the trailing gutter space every cell
/// renders — what a column has to be to hold this cell. Passing no glyphs
/// measures the gutter space alone.
fn measure_glyphs(shaper: &mut Shaper, glyphs: &[Glyph]) -> f32 {
    let mut gs = glyphs.to_vec();
    gs.push(Glyph {
        ch: ' ',
        style: CoreStyle::default(),
        src: 0,
        stop: true,
    });
    f32::from(shaper.shape(&gs, None).width)
}

/// The UTF-8 byte offset a UTF-16 offset into `source` names — [`Editor::offset_from_utf16`]'s
/// logic, split out to be unit-testable without a live document.
fn utf16_to_utf8(source: &str, target: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for ch in source.chars() {
        if utf16 >= target {
            break;
        }
        utf16 += ch.len_utf16();
        utf8 += ch.len_utf8();
    }
    utf8
}

/// The inverse of [`utf16_to_utf8`] — see [`Editor::offset_to_utf16`].
fn utf8_to_utf16(source: &str, target: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for ch in source.chars() {
        if utf8 >= target {
            break;
        }
        utf8 += ch.len_utf8();
        utf16 += ch.len_utf16();
    }
    utf16
}

// ── the system clipboard ─────────────────────────────────────────────────────
//
// arboard, and deliberately not gpui's clipboard, which cannot carry HTML: a
// `ClipboardEntry` is `String | Image | ExternalPaths`, and the `metadata` a
// `ClipboardString` also holds is written by the macOS backend as a *private*
// pasteboard type beside `NSPasteboardTypeString` — never
// `NSPasteboardTypeHTML`, so no other application can ever read it. A rich
// clipboard is exactly the flavor gpui has no way to publish, so the desktop
// build goes around it to the same `NSPasteboard` arboard talks to. That it is
// also the crate leaf-tui uses is the other half of the win: one clipboard
// implementation for both frontends instead of a frontend divergence.
//
// Mobile keeps gpui's clipboard (arboard has no iOS backend — see the `desktop`
// feature), and so keeps the plain flavor only. `cx` is what makes that
// fallback possible and is unused on the desktop.
//
// A fresh `Clipboard` per operation and every failure degrading to doing
// nothing, the same discipline as leaf-tui.

#[cfg(feature = "desktop")]
fn set_clipboard(plain: String, html: Option<String>, _cx: &mut App) {
    let Ok(mut clipboard) = arboard::Clipboard::new() else { return };
    // One clear-and-set writes both flavors, so a copy can't leave a stale HTML
    // flavor from an earlier one behind for a paste to find and prefer.
    let _ = match html {
        Some(html) => clipboard.set().html(html, Some(plain)),
        None => clipboard.set_text(plain),
    };
}

#[cfg(not(feature = "desktop"))]
fn set_clipboard(plain: String, _html: Option<String>, cx: &mut App) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(plain));
}

#[cfg(feature = "desktop")]
fn get_clipboard_text(_cx: &mut App) -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

#[cfg(not(feature = "desktop"))]
fn get_clipboard_text(cx: &mut App) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

#[cfg(feature = "desktop")]
fn get_clipboard_html(_cx: &mut App) -> Option<String> {
    arboard::Clipboard::new().ok()?.get().html().ok()
}

#[cfg(not(feature = "desktop"))]
fn get_clipboard_html(_cx: &mut App) -> Option<String> {
    None // gpui's clipboard has no HTML flavor to prefer.
}

/// Which source bytes an IME composition step replaces — the subtle half of
/// `replace_and_mark_text_in_range`, split out to be testable on its own.
///
/// The three inputs disagree about their basis on purpose, because AppKit does:
/// `range_utf16` is relative to the *marked* range when a composition is
/// already up (that's `setMarkedText:`'s replacement range) and absolute when
/// one isn't, while `marked` and `selection` are already source bytes. Read it
/// as absolute in the relative case and the second keystroke of every
/// composition lands somewhere else in the document.
fn marked_replace_range(
    source: &str,
    range_utf16: Option<Range<usize>>,
    marked: Option<Range<usize>>,
    selection: Range<usize>,
) -> Range<usize> {
    match (range_utf16, marked) {
        (Some(r), Some(marked)) => {
            let base = utf8_to_utf16(source, marked.start);
            utf16_to_utf8(source, base + r.start)..utf16_to_utf8(source, base + r.end)
        }
        (Some(r), None) => utf16_to_utf8(source, r.start)..utf16_to_utf8(source, r.end),
        // No replacement range with a composition up means "all of it".
        (None, Some(marked)) => marked,
        // The first keystroke of a composition replaces whatever was selected.
        (None, None) => selection,
    }
}

/// A glyph run's concatenated text and its per-glyph cumulative byte offsets
/// (`chars + 1` entries, the last being the total length).
fn char_bytes(glyphs: &[Glyph]) -> Vec<usize> {
    let mut char_byte = vec![0usize];
    let mut acc = 0usize;
    for g in glyphs {
        acc += g.ch.len_utf8();
        char_byte.push(acc);
    }
    char_byte
}

/// Locate the caret's visual `(row, glyph column)` for source offset `caret`
/// against the painted rows. Thin wrapper over [`locate_caret_core`], which is
/// split out to be unit-testable without a live text system.
fn locate_caret(rows: &[RowLayout], caret: usize) -> (usize, usize) {
    let srcs: Vec<&[usize]> = rows.iter().map(|r| r.char_srcs.as_slice()).collect();
    let ends: Vec<usize> = rows.iter().map(|r| r.end_src).collect();
    locate_caret_core(&srcs, &ends, caret)
}

/// Map a source offset to `(visual row, glyph column)`. The column may equal the
/// row's glyph count, meaning "just past the last glyph".
///
/// The rule is the *nearest* offset at or past the caret, across every row —
/// each row offering its first glyph at or past it, or failing that its own end.
/// Taking the first row that has any such glyph would be the same answer on
/// ordinary prose, where rows ascend, and the wrong one in a table: a wrapped
/// cell puts column 1's second line *below* column 2's first while holding
/// smaller offsets, so the first row to match is the cell to the right rather
/// than the line underneath. This mirrors `leaf-core`'s `pos_of_offset`, which
/// the TUI has always ridden.
///
/// A tie goes to the *later* row. The only offset two rows both hold is a
/// soft-wrap boundary — the row above ends where the row below opens — and it
/// belongs to the row below: the row above's far edge is a phantom the caret can
/// be drawn at but never sent to. A block gap can't tie, since its blank row is
/// anchored past the end of the block above it.
fn locate_caret_core(row_srcs: &[&[usize]], row_end: &[usize], caret: usize) -> (usize, usize) {
    let mut best: Option<(usize, usize, usize)> = None; // (src, row, glyph column)
    for (r, srcs) in row_srcs.iter().enumerate() {
        let cand = srcs
            .iter()
            .position(|&s| s >= caret)
            .map(|gi| (srcs[gi], r, gi))
            .or_else(|| (row_end[r] >= caret).then_some((row_end[r], r, srcs.len())));
        if let Some(c) = cand
            && best.is_none_or(|b| c.0 <= b.0)
        {
            best = Some(c);
        }
        // A row's first glyph never opens earlier than the row above's — true
        // even across a table's wrapped cells, whose lines run downward. So once
        // a row opens past the best found, no later row can beat it.
        if let (Some(b), Some(&first)) = (best, srcs.first())
            && first > b.0
        {
            break;
        }
    }
    match best {
        Some((_, r, gi)) => (r, gi),
        None => {
            let r = row_srcs.len().saturating_sub(1);
            (r, row_srcs.get(r).map(|s| s.len()).unwrap_or(0))
        }
    }
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // The pixel-wrapped row count is only known once prepaint has laid the
        // text out, so reserve height from the last paint's count (self-correcting
        // by one frame, like `last_bounds`). Before the first paint, one row.
        let (n, bottom_inset) = {
            let e = self.editor.read(cx);
            (e.last_row_count.max(1), e.bottom_inset)
        };
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        // Reserve the rows' height plus the host bottom inset (keyboard) as extra
        // scroll room, so the caret can be scrolled clear of the keyboard even
        // when the document itself is shorter than the screen. The true height is
        // the last paint's sum of (now variable) row heights; before the first
        // paint, fall back to one line per reserved row.
        let last_height = self
            .editor
            .read(cx)
            .last_row_tops
            .last()
            .copied()
            .filter(|h| *h > px(0.0))
            .unwrap_or(window.line_height() * n as f32);
        style.size.height = (last_height + bottom_inset).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let font: Font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let wrap_px = f32::from(bounds.size.width);

        // No document open: nothing to lay out (the `+` button is rendered
        // elsewhere, so this element is empty). Return a single blank row.
        if self.editor.read(cx).doc.is_none() {
            let shaped = Rc::new(
                window
                    .text_system()
                    .shape_line("".into(), font_size, &[], None),
            );
            let rows = Rc::new(vec![RowLayout::prose(shaped, Vec::new(), vec![0], 0, line_height)]);
            let tops = Rc::new(row_tops(&rows));
            let row_x = Rc::new(vec![px(0.0); rows.len()]);
            return Prepaint {
                rows,
                tops,
                row_x,
                line_height,
                cursor: None,
                selections: Vec::new(),
                table_fills: Vec::new(),
                table_borders: Vec::new(),
                code_fills: Vec::new(),
                code_boxes: Vec::new(),
                code_labels: Vec::new(),
                code_label_h: line_height,
                images: Vec::new(),
            };
        }

        // The WYSIWYG map must be current before we read caret/selection, since
        // both ride it. `build_visual_unwrapped` is itself cached on the
        // document's revision, so this is free unless the text moved.
        let view = self.editor.read(cx).doc.as_ref().unwrap().view;
        if view == View::Wysiwyg {
            self.editor
                .update(cx, |e, _| e.doc.as_mut().unwrap().build_visual_unwrapped());
        }

        let (key, sel, caret, style, marked, cached, doc_dir) = {
            let editor = self.editor.read(cx);
            let doc = editor.doc.as_ref().unwrap();
            let key = LayoutKey {
                revision: doc.revision(),
                width: wrap_px.to_bits(),
                source_view: doc.view == View::Source,
                marked: editor.marked_range.clone(),
            };
            // Reusing the rows is only sound if they're really there: the first
            // paint has the key unset.
            let cached = (editor.layout_key.as_ref() == Some(&key) && !editor.last_rows.is_empty())
                .then(|| {
                    (
                        editor.last_rows.clone(),
                        editor.last_geoms.clone(),
                        editor.last_code_geoms.clone(),
                        editor.last_image_geoms.clone(),
                    )
                });
            // The document's directory — what a relative image path resolves
            // against. Empty for an untitled buffer, where a relative path can't
            // resolve and falls back to the text placeholder.
            let doc_dir = doc.path.parent().map(|p| p.to_path_buf()).filter(|p| !p.as_os_str().is_empty());
            (
                key,
                doc.selection(),
                doc.caret,
                editor.style.clone(),
                editor.marked_range.clone(),
                cached,
                doc_dir,
            )
        };
        let (caret_color, selection_color) = (style.caret, style.selection);

        // Shape the document, unless the last paint already shaped this exact
        // one. The caret and selection below are recomputed either way — they
        // move without the text moving, and they're cheap next to shaping.
        let (rows, geoms, code_geoms, image_geoms) = match cached {
            Some(hit) => hit,
            None => {
                // Gather the logical lines (glyphs owned) so we can shape them
                // with a mutable window borrow after the document borrow is
                // dropped. A logical line is a whole paragraph (WYSIWYG) or a
                // source line (Source); the pixel wrap that follows turns each
                // into one or more visual rows.
                let (logical_lines, code_langs) = {
                    let editor = self.editor.read(cx);
                    let doc = editor.doc.as_ref().unwrap();
                    // The language label of each code block, in the same order
                    // `gather_logical` numbers them, so a `CodeGeom` can carry its
                    // own once built.
                    let langs: Vec<Option<String>> =
                        doc.vmap.code_blocks.iter().map(|c| c.lang.clone()).collect();
                    (gather_logical(doc), langs)
                };
                // Decode every block image up front, before the shaper borrows
                // the window — decoding needs only `cx` and the editor's cache.
                // Each raster is cached on the editor by resolved path, so a
                // relayout neither re-reads nor re-decodes it. `loaded` maps a
                // destination to its raster for the shaping loop; `None` means the
                // destination isn't a loadable local file (or failed to decode),
                // and the row falls back to core's `🖼 alt` text placeholder.
                // The picture source to load is theme-dependent: a `<picture>`
                // with a `prefers-color-scheme` `<source>` gets the dark or light
                // file per the window's appearance. Keyed (and reserved) by the
                // canonical `destination` regardless, so the shaping lookup below
                // is unchanged and a theme switch just re-picks on the next paint.
                let scheme = window_color_scheme(window);
                let loaded: HashMap<String, Option<Arc<RenderImage>>> = {
                    let mut loaded = HashMap::new();
                    for logical in &logical_lines {
                        let Logical::Image { info, .. } = logical else { continue };
                        if loaded.contains_key(&info.destination) {
                            continue;
                        }
                        let img = match resolve_image_path(info.resolve(scheme), doc_dir.as_deref()) {
                            Some(path) => self.editor.update(cx, |e, _| {
                                e.image_cache
                                    .entry(path.clone())
                                    .or_insert_with(|| load_image_file(&path))
                                    .clone()
                            }),
                            None => None,
                        };
                        loaded.insert(info.destination.clone(), img);
                    }
                    loaded
                };
                // The shape cache rides with the editor between paints; take
                // it for the duration, since shaping needs the window mutably
                // and the editor can't be borrowed across that.
                let (prev, prev_breaks) = self.editor.update(cx, |e, _| {
                    (
                        std::mem::take(&mut e.shape_cache),
                        std::mem::take(&mut e.break_cache),
                    )
                });
                // The families and palette a run is styled with, and the ratio
                // that turns a font size into a row height — the metrics the
                // shaper reads to style each line by its role (see
                // [`Shaper::line_size`] and [`text_run`]).
                let styler = RunStyle {
                    body: font.clone(),
                    mono: gpui::font(style.mono_font_family.clone()),
                    text: style.text,
                    link: style.link,
                    muted: style.muted,
                    mark_bg: style.mark_background,
                    code_bg: style.code_background,
                };
                let line_ratio = (f32::from(line_height) / f32::from(font_size).max(1.0)).max(1.0);
                let mut shaper = Shaper {
                    window,
                    styler,
                    body_size: font_size,
                    heading_scale: style.heading_scale,
                    line_ratio,
                    prev,
                    fresh: HashMap::new(),
                    breaks: HashMap::new(),
                    prev_breaks,
                };

                let mut rows: Vec<RowLayout> = Vec::new();
                let mut geoms: Vec<TableGeom> = Vec::new();
                let mut code_geoms: Vec<CodeGeom> = Vec::new();
                let mut image_geoms: Vec<ImageGeom> = Vec::new();
                for logical in &logical_lines {
                    match logical {
                        Logical::Line { glyphs, end_src, code, decoration } => {
                            let before = rows.len();
                            // A code line never wraps — it scrolls inside its box —
                            // so it's laid out at an unbounded width and stays one
                            // row. Prose wraps at the element width as before.
                            let w = if code.is_some() { f32::INFINITY } else { wrap_px };
                            // A block-gap separator is drawn short — paragraph
                            // spacing, not a full blank line.
                            let gap = decoration
                                .then(|| px(f32::from(line_height) * style.block_gap_scale));
                            wrap_logical(&mut shaper, glyphs, *end_src, w, marked.as_ref(), gap, &mut rows);
                            if let Some(id) = code {
                                // Lines of one block are consecutive, so the first
                                // opens its span and the rest extend it.
                                if *id == code_geoms.len() {
                                    let lang = code_langs.get(*id).cloned().flatten();
                                    code_geoms.push(CodeGeom { rows: before..rows.len(), lang });
                                } else {
                                    code_geoms[*id].rows.end = rows.len();
                                }
                            }
                        }
                        Logical::Table(info) => {
                            if let Some(g) =
                                layout_table(&mut shaper, info, wrap_px, marked.as_ref(), &mut rows)
                            {
                                geoms.push(g);
                            }
                        }
                        Logical::Image { info, glyphs, end_src } => {
                            match loaded.get(&info.destination).cloned().flatten() {
                                Some(image) => {
                                    // A loaded raster: reserve one row as tall as
                                    // its fitted box (plus padding), carrying the
                                    // caret stop in front of the image and one just
                                    // past it, and record the geom painted over it.
                                    let box_size = image_box_size(image.size(0), wrap_px);
                                    let height = box_size.height + px(2.0 * IMAGE_PAD_Y);
                                    let start = glyphs.first().map_or(*end_src, |g| g.src);
                                    let shaped = shaper.empty();
                                    rows.push(RowLayout::prose(
                                        shaped,
                                        vec![start],
                                        vec![0, 0],
                                        *end_src,
                                        height,
                                    ));
                                    image_geoms.push(ImageGeom {
                                        row: rows.len() - 1,
                                        image,
                                        size: box_size,
                                    });
                                }
                                None => {
                                    // Not a loadable local image: fall back to
                                    // core's `🖼 alt` placeholder, wrapped like any
                                    // other prose line.
                                    wrap_logical(
                                        &mut shaper,
                                        glyphs,
                                        *end_src,
                                        wrap_px,
                                        marked.as_ref(),
                                        None,
                                        &mut rows,
                                    );
                                }
                            }
                        }
                    }
                }
                if rows.is_empty() {
                    let shaped = shaper.empty();
                    let h = shaper.row_height(&[]);
                    rows.push(RowLayout::prose(shaped, Vec::new(), vec![0], 0, h));
                }

                // Whatever is still in `prev` was not used by this paint: text
                // that has been edited away. Only `fresh` goes back.
                let (shapes, breaks) = (shaper.fresh, shaper.breaks);
                let (rows, geoms, code_geoms, image_geoms) = (
                    Rc::new(rows),
                    Rc::new(geoms),
                    Rc::new(code_geoms),
                    Rc::new(image_geoms),
                );
                self.editor.update(cx, |e, _| {
                    e.last_rows = rows.clone();
                    e.last_geoms = geoms.clone();
                    e.last_code_geoms = code_geoms.clone();
                    e.last_image_geoms = image_geoms.clone();
                    e.layout_key = Some(key);
                    e.last_row_count = rows.len();
                    e.shape_cache = shapes;
                    e.break_cache = breaks;
                });
                (rows, geoms, code_geoms, image_geoms)
            }
        };

        let left = bounds.left();
        let top = bounds.top();
        // Rows are stacked by their own heights, not a uniform line height —
        // `tops[r]` is where row `r` starts, `tops[r + 1]` where it ends.
        let tops = Rc::new(row_tops(&rows));
        let row_top = |row: usize| top + tops[row];
        let row_h = |row: usize| tops[row + 1] - tops[row];

        // Caret: locate its visual row/column from the source offset against the
        // painted rows — the pixel wrap means caret_pos()'s paragraph grid no
        // longer matches the rows on screen.
        let (cr, cgi) = locate_caret(&rows, caret);
        let cr = cr.min(rows.len() - 1);

        // Code blocks scroll horizontally inside their box rather than wrapping,
        // and — like a table — the box is only as wide as its widest line rather
        // than the whole editor. Every code row's text is indented `CODE_PAD_X`;
        // the one block holding the caret is then scrolled left just enough to
        // keep the caret a margin clear of the box's right edge (every other block
        // shows from column 0). `row_x[r]` is the net x added to row `r`'s text —
        // a code row's indent-minus-scroll, zero for prose. It's recomputed here,
        // not baked into the cached rows, because the caret moves without the rows
        // re-shaping. `code_boxes` pairs each block's rows with the box rect its
        // border, fill, and text-clip are drawn from.
        let avail = f32::from(bounds.size.width);
        // The widest laid-out row in a block, in pixels — what the box hugs.
        let block_content_w = |g: &CodeGeom| -> f32 {
            g.rows
                .clone()
                .map(|r| f32::from(rows[r].segments.iter().map(|s| s.x + s.shaped.width).max().unwrap_or(px(0.0))))
                .fold(0.0f32, f32::max)
        };
        // Shape each block's language label once, so its width feeds the box
        // width (the box never cuts its own label) and the paint step just draws
        // it. A block without a language shapes nothing.
        let label_size = font_size * CODE_LABEL_SCALE;
        let mut code_labels: Vec<Option<Rc<ShapedLine>>> = Vec::with_capacity(code_geoms.len());
        for g in code_geoms.iter() {
            let shaped = g.lang.as_ref().map(|lang| {
                let text: SharedString = format!(" {lang} ").into();
                let run = gpui::TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: style.muted,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                Rc::new(window.text_system().shape_line(text, label_size, &[run], None))
            });
            code_labels.push(shaped);
        }
        // Each block's box width: its content (or its label, whichever is wider)
        // plus the padding either side, capped at the editor width.
        let box_w = |bi: usize, g: &CodeGeom| -> f32 {
            let label_w = code_labels[bi].as_ref().map_or(0.0, |s| f32::from(s.width) + CODE_PAD_X);
            (block_content_w(g).max(label_w) + 2.0 * CODE_PAD_X).min(avail).max(2.0 * CODE_PAD_X)
        };

        let caret_block = code_geoms.iter().position(|g| g.rows.contains(&cr));
        let caret_off = caret_block.map_or(0.0, |bi| {
            let inner_w = (box_w(bi, &code_geoms[bi]) - 2.0 * CODE_PAD_X).max(1.0);
            (f32::from(rows[cr].x_at(cgi)) - inner_w + CODE_CARET_MARGIN).max(0.0)
        });
        let mut row_x = vec![px(0.0); rows.len()];
        let mut code_boxes: Vec<(Range<usize>, Bounds<Pixels>)> = Vec::new();
        let mut code_labels_out: Vec<(Rc<ShapedLine>, Point<Pixels>, Bounds<Pixels>)> = Vec::new();
        let label_h = label_size * 1.3;
        for (bi, g) in code_geoms.iter().enumerate() {
            let off = if Some(bi) == caret_block { caret_off } else { 0.0 };
            for r in g.rows.clone() {
                row_x[r] = px(CODE_PAD_X - off);
            }
            // The box grows a little past its rows into the blank separator lines
            // above and below, and is only as wide as its content.
            let box_top = row_top(g.rows.start) - px(CODE_PAD_Y);
            let box_bottom = top + tops[g.rows.end] + px(CODE_PAD_Y);
            let rect = Bounds::from_corners(
                point(left, box_top),
                point(left + px(box_w(bi, g)), box_bottom),
            );
            code_boxes.push((g.rows.clone(), rect));
            // The language label straddles the top border as a little chip, its
            // own background cutting the border the way a fieldset legend does.
            if let Some(shaped) = &code_labels[bi] {
                let origin = point(rect.left() + px(CODE_PAD_X), box_top - label_h * 0.5);
                let chip = Bounds::new(origin, size(shaped.width, label_h));
                code_labels_out.push((shaped.clone(), origin, chip));
            }
        }
        let row_x = Rc::new(row_x);

        let caret_x = rows[cr].x_at(cgi) + row_x[cr];
        let cursor = if sel.is_none() {
            Some(fill(
                Bounds::new(point(left + caret_x, row_top(cr)), size(px(2.0), row_h(cr))),
                caret_color,
            ))
        } else {
            None
        };

        // Selection: highlight, per segment, the run of characters whose source
        // byte falls in the selection — visible-space, so hidden delimiters are
        // skipped. Per segment rather than per row because a table row's cells
        // are far apart: one quad spanning them would paint over the borders and
        // the gutters between, claiming text that isn't selected.
        let mut selections = Vec::new();
        if let Some((s0, s1)) = sel {
            for (r, row) in rows.iter().enumerate() {
                for (si, seg) in row.segments.iter().enumerate() {
                    let upto = row
                        .segments
                        .get(si + 1)
                        .map_or(row.char_srcs.len(), |n| n.first);
                    let mut a: Option<usize> = None;
                    let mut b = 0usize;
                    for i in seg.first..upto {
                        let src = row.char_srcs[i];
                        if src >= s0 && src < s1 {
                            a.get_or_insert(i);
                            b = i + 1;
                        }
                    }
                    if let Some(a) = a {
                        selections.push(fill(
                            Bounds::from_corners(
                                point(left + row.x_in(si, a) + row_x[r], row_top(r)),
                                point(left + row.x_in(si, b) + row_x[r], row_top(r) + row_h(r)),
                            ),
                            selection_color,
                        ));
                    }
                }
            }
        }

        // Chrome is quads, not text — rebuilt each paint because it's cheap and
        // rides `bounds`, which the cache key doesn't cover.
        let (mut table_fills, mut table_borders) = (Vec::new(), Vec::new());
        for g in geoms.iter() {
            let (f, b) = table_chrome(g, left, top, &tops, &style);
            table_fills.extend(f);
            table_borders.extend(b);
        }

        // A code block's box: one rounded quad, a tinted fill under a thin
        // border, painted before the selection and text so both land on top.
        let code_fills: Vec<PaintQuad> = code_boxes
            .iter()
            .map(|(_, rect)| {
                quad(
                    *rect,
                    px(CODE_RADIUS),
                    style.code_background,
                    px(CODE_BORDER),
                    style.code_border,
                    BorderStyle::Solid,
                )
            })
            .collect();

        // Each block image's on-screen box: its reserved row's top plus the
        // vertical padding, left-aligned, at the fitted size. The rect rides
        // `bounds` (via the row tops), so it's resolved here rather than cached.
        let images: Vec<(Bounds<Pixels>, Arc<RenderImage>)> = image_geoms
            .iter()
            .filter(|g| g.row + 1 < tops.len())
            .map(|g| {
                let origin = point(left, row_top(g.row) + px(IMAGE_PAD_Y));
                (Bounds::new(origin, g.size), g.image.clone())
            })
            .collect();

        Prepaint {
            rows,
            tops,
            row_x,
            line_height,
            cursor,
            selections,
            table_fills,
            table_borders,
            code_fills,
            code_boxes,
            code_labels: code_labels_out,
            code_label_h: label_h,
            images,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );

        // A code block's box, then a table's fills, all go under the selection;
        // a table's rules go over it, so a selection running across a cell
        // boundary doesn't swallow the grid.
        for quad in prepaint.code_fills.drain(..) {
            window.paint_quad(quad);
        }
        // The language chips sit over the box border they cut into, their own
        // background masking the border where the label crosses it.
        let code_label_h = prepaint.code_label_h;
        let code_bg = self.editor.read(cx).style.code_background;
        for (shaped, origin, chip) in prepaint.code_labels.drain(..) {
            window.paint_quad(fill(chip, code_bg));
            shaped.paint(origin, code_label_h, TextAlign::Left, None, window, cx).ok();
        }
        for quad in prepaint.table_fills.drain(..) {
            window.paint_quad(quad);
        }
        for quad in prepaint.selections.drain(..) {
            window.paint_quad(quad);
        }
        for quad in prepaint.table_borders.drain(..) {
            window.paint_quad(quad);
        }

        let left = bounds.left();
        let top = bounds.top();
        let tops = prepaint.tops.clone();
        let row_x = prepaint.row_x.clone();
        // Which box, if any, clips a given row's text (a scrolled code line must
        // not spill past its box). `code_boxes` is tiny, so a linear scan per row
        // is cheaper than a per-row lookup table.
        let clip_of = |r: usize| -> Option<Bounds<Pixels>> {
            prepaint
                .code_boxes
                .iter()
                .find(|(rows, _)| rows.contains(&r))
                .map(|(_, rect)| *rect)
        };
        for (r, row) in prepaint.rows.iter().enumerate() {
            // Each row paints at its own top and its own height — a shaped line is
            // painted at the height it was measured to, so a heading's larger
            // glyphs sit in a proportionally taller row. A code row is shifted by
            // its box's indent-minus-scroll and clipped to the box.
            let y = top + tops[r];
            let dx = row_x[r];
            let paint_row = |window: &mut Window, cx: &mut App| {
                for seg in &row.segments {
                    seg.shaped
                        .paint(point(left + seg.x + dx, y), row.height, TextAlign::Left, None, window, cx)
                        .ok();
                }
            };
            match clip_of(r) {
                Some(rect) => window.with_content_mask(Some(ContentMask { bounds: rect }), |w| {
                    paint_row(w, cx)
                }),
                None => paint_row(window, cx),
            }
        }

        // Block images, painted over their reserved (empty) rows. `paint_image`
        // uploads to the sprite atlas lazily and caches by the frame's id, so
        // reusing the same `Arc<RenderImage>` across frames re-uploads nothing.
        for (rect, image) in prepaint.images.drain(..) {
            window
                .paint_image(rect, Corners::default(), image, 0, false)
                .ok();
        }

        // The blink phase decides whether the caret paints this frame; `render`'s
        // `sync_blink` keeps it solid through a run of typing. `!= Some(false)`
        // rather than `== Some(true)`: a frame that beats the first `sync_blink`
        // (phase `None`) should show a caret, not wait half a second for one.
        let blink_on = self.editor.read(cx).blink_phase != Some(false);
        if blink_on
            && focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        // The rows are already the editor's — prepaint stored them under their
        // key. Only what the mouse needs to turn a pixel into a row is left: the
        // cumulative tops (for hit-testing y → row) and a body line height (for
        // page up/down).
        self.editor.update(cx, |editor, _| {
            editor.last_line_height = prepaint.line_height;
            editor.last_row_tops = tops;
            editor.last_row_x = row_x;
            editor.last_bounds = Some(bounds);
        });
    }
}

impl Render for Editor {
    /// Renders just the editing surface — a focusable, scrollable text body with
    /// the caret, selection, and an optional right-click menu. No window chrome:
    /// a host places this inside its own layout (see the `leaf` binary for the app that
    /// wraps it with a header and file-open UI).
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_blink(window, cx);
        let style = self.style.clone();
        let (name, dirty) = (self.file_name(), self.is_dirty());
        div()
            .flex()
            .flex_col()
            .size_full()
            .font_family(style.font_family.clone())
            .bg(style.background)
            .text_color(style.text)
            .key_context("Editor")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            // Every document key binding, action, and mouse listener lives
            // behind this one gate: while a modal is up (the prompt, or a
            // dialog), none of them are even registered, so a resolved
            // keystroke that would otherwise hit one of these finds no
            // listener anywhere in the tree, falls through gpui's action
            // dispatch untouched, and reaches the prompt's own raw
            // `on_key_down` instead (see `prompt_key_down`). Simpler and far
            // less error-prone than threading a guard through every handler
            // below — and a dialog's question can't be answered by typing into
            // the document behind it.
            .when(self.prompt.is_none() && self.dialog.is_none(), |el| {
                el.on_action(cx.listener(Self::left))
                    .on_action(cx.listener(Self::right))
                    .on_action(cx.listener(Self::up))
                    .on_action(cx.listener(Self::down))
                    .on_action(cx.listener(Self::select_left))
                    .on_action(cx.listener(Self::select_right))
                    .on_action(cx.listener(Self::select_up))
                    .on_action(cx.listener(Self::select_down))
                    .on_action(cx.listener(Self::home))
                    .on_action(cx.listener(Self::end))
                    .on_action(cx.listener(Self::select_home))
                    .on_action(cx.listener(Self::select_end))
                    .on_action(cx.listener(Self::backspace))
                    .on_action(cx.listener(Self::delete))
                    .on_action(cx.listener(Self::newline))
                    .on_action(cx.listener(Self::indent))
                    .on_action(cx.listener(Self::outdent))
                    .on_action(cx.listener(Self::delete_to_line_start))
                    .on_action(cx.listener(Self::delete_to_line_end))
                    .on_action(cx.listener(Self::toggle_bold))
                    .on_action(cx.listener(Self::toggle_italic))
                    .on_action(cx.listener(Self::toggle_strikethrough))
                    .on_action(cx.listener(Self::toggle_underline))
                    .on_action(cx.listener(Self::toggle_view))
                    .on_action(cx.listener(Self::save))
                    .on_action(cx.listener(Self::save_as))
                    .on_action(cx.listener(Self::new_document))
                    .on_action(cx.listener(Self::undo))
                    .on_action(cx.listener(Self::redo))
                    .on_action(cx.listener(Self::doc_start))
                    .on_action(cx.listener(Self::doc_end))
                    .on_action(cx.listener(Self::select_doc_start))
                    .on_action(cx.listener(Self::select_doc_end))
                    .on_action(cx.listener(Self::page_up))
                    .on_action(cx.listener(Self::page_down))
                    .on_action(cx.listener(Self::select_page_up))
                    .on_action(cx.listener(Self::select_page_down))
                    .on_action(cx.listener(Self::move_word_left))
                    .on_action(cx.listener(Self::move_word_right))
                    .on_action(cx.listener(Self::select_word_left))
                    .on_action(cx.listener(Self::select_word_right))
                    .on_action(cx.listener(Self::delete_word_back))
                    .on_action(cx.listener(Self::delete_word_forward))
                    .on_action(cx.listener(Self::select_all))
                    .on_action(cx.listener(Self::toggle_code))
                    .on_action(cx.listener(Self::toggle_mark))
                    .on_action(cx.listener(Self::set_paragraph))
                    .on_action(cx.listener(Self::heading1))
                    .on_action(cx.listener(Self::heading2))
                    .on_action(cx.listener(Self::heading3))
                    .on_action(cx.listener(Self::heading4))
                    .on_action(cx.listener(Self::heading5))
                    .on_action(cx.listener(Self::heading6))
                    .on_action(cx.listener(Self::toggle_blockquote))
                    .on_action(cx.listener(Self::toggle_bullet_list))
                    .on_action(cx.listener(Self::toggle_ordered_list))
                    .on_action(cx.listener(Self::insert_link))
                    .on_action(cx.listener(Self::set_language))
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::cut))
                    .on_action(cx.listener(Self::paste))
                    .on_action(cx.listener(Self::paste_as_plain_text))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
            })
            .when_some(self.context_menu, |el, pos| {
                el.child(Self::render_context_menu(pos, &mut *cx))
            })
            .when_some(self.prompt.as_ref(), |el, prompt| {
                el.child(Self::render_prompt(prompt, &mut *cx))
            })
            .when_some(self.dialog, |el, dialog| {
                el.child(Self::render_dialog(dialog, name.clone(), dirty, &mut *cx))
            })
            .child(
                div()
                    .id("body")
                    .flex_1()
                    .p_3()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .text_size(style.font_size)
                    .line_height(style.line_height)
                    .child(TextElement {
                        editor: cx.entity(),
                    }),
            )
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// A [`RunStyle`] over `body` with the default theme's palette — the styler the
/// shaping and run tests build their shapes with.
#[cfg(test)]
fn test_run_style(body: Font) -> RunStyle {
    let theme = EditorStyle::default();
    RunStyle {
        body,
        mono: gpui::font("Menlo"),
        text: theme.text,
        link: theme.link,
        muted: theme.muted,
        mark_bg: theme.mark_background,
        code_bg: theme.code_background,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Glyph, build_runs, locate_caret_core, marked_replace_range, test_run_style, utf16_to_utf8,
        utf8_to_utf16,
    };
    use leaf_core::style::Style as CoreStyle;

    // Two paragraphs, the first wrapped across two visual rows:
    //   row 0: "one two "  srcs 0..7   end 8   (soft-wrap boundary at 8)
    //   row 1: "three"     srcs 8..12  end 13
    //   row 2: ""          (blank separator)   end 14
    //   row 3: "def"       srcs 15..17 end 18
    fn fixture() -> (Vec<Vec<usize>>, Vec<usize>) {
        (
            vec![
                vec![0, 1, 2, 3, 4, 5, 6, 7],
                vec![8, 9, 10, 11, 12],
                vec![],
                vec![15, 16, 17],
            ],
            vec![8, 13, 14, 18],
        )
    }

    fn locate(caret: usize) -> (usize, usize) {
        let (srcs, ends) = fixture();
        let refs: Vec<&[usize]> = srcs.iter().map(|v| v.as_slice()).collect();
        locate_caret_core(&refs, &ends, caret)
    }

    #[test]
    fn caret_inside_a_row_lands_in_that_row() {
        assert_eq!(locate(0), (0, 0));
        assert_eq!(locate(3), (0, 3));
        assert_eq!(locate(10), (1, 2));
        assert_eq!(locate(16), (3, 1));
    }

    #[test]
    fn soft_wrap_boundary_biases_to_the_next_rows_start() {
        // Offset 8 ends row 0 and starts row 1 — it must resolve to row 1 col 0,
        // so the caret rides the wrapped line instead of the first row's far edge.
        assert_eq!(locate(8), (1, 0));
    }

    #[test]
    fn paragraph_end_stays_at_that_rows_end() {
        // Offset 13 ends "three"; the next row is a blank separator (offset 14),
        // not a soft-wrap continuation, so the caret stays at row 1's end.
        assert_eq!(locate(13), (1, 5));
        // The blank separator line itself is reachable.
        assert_eq!(locate(14), (2, 0));
    }

    /// A two-column table whose first cell wraps, so the rows run *out* of source
    /// order — the shape that breaks a first-match scan:
    ///   row 0: "aa" (col 1, srcs 0..1)  "cc" (col 2, srcs 6..7)   end 8
    ///   row 1: "bb" (col 1, srcs 3..4)                            end 5
    /// Offset 3 is on row 1, but row 0 is the first row holding an offset at or
    /// past it (6, over in column 2).
    fn table_fixture() -> (Vec<Vec<usize>>, Vec<usize>) {
        (vec![vec![0, 1, 6, 7], vec![3, 4]], vec![8, 5])
    }

    fn locate_table(caret: usize) -> (usize, usize) {
        let (srcs, ends) = table_fixture();
        let refs: Vec<&[usize]> = srcs.iter().map(|v| v.as_slice()).collect();
        locate_caret_core(&refs, &ends, caret)
    }

    #[test]
    fn a_wrapped_table_cell_puts_the_caret_below_not_in_the_next_column() {
        // The bug a first-match scan has: it would answer (0, 2) — the caret
        // teleports into column 2 the moment you step into a wrapped cell's
        // second line.
        assert_eq!(locate_table(3), (1, 0));
        assert_eq!(locate_table(4), (1, 1));
        // Column 2's own text still resolves to column 2.
        assert_eq!(locate_table(6), (0, 2));
    }

    #[test]
    fn a_table_cells_end_resolves_to_that_cell_not_a_later_row() {
        // Offset 5 ends "bb" on row 1. Row 0 offers nothing at or past 5 before
        // its own end (8), so the nearer candidate is row 1's end.
        assert_eq!(locate_table(5), (1, 2));
    }

    #[test]
    fn caret_at_document_end_lands_on_the_last_row() {
        assert_eq!(locate(18), (3, 3));
    }

    // ── UTF-8 ⇄ UTF-16 (the gpui input seam) ────────────────────────────────

    #[test]
    fn utf16_offsets_round_trip_through_multibyte_text() {
        // "é" is 2 UTF-8 bytes / 1 UTF-16 unit; "€" is 3 / 1.
        let s = "aé€b";
        for (utf8, utf16) in [(0, 0), (1, 1), (3, 2), (6, 3), (7, 4)] {
            assert_eq!(utf8_to_utf16(s, utf8), utf16, "utf8 {utf8} → utf16");
            assert_eq!(utf16_to_utf8(s, utf16), utf8, "utf16 {utf16} → utf8");
        }
    }

    #[test]
    fn an_astral_char_is_one_utf16_surrogate_pair_not_one_unit() {
        // 😀 is 4 UTF-8 bytes and *two* UTF-16 units — the case that makes the
        // two offset spaces disagree by more than a constant factor.
        let s = "a😀b";
        assert_eq!(utf8_to_utf16(s, 5), 3);
        assert_eq!(utf16_to_utf8(s, 3), 5);
        assert_eq!(utf8_to_utf16(s, 6), 4);
    }

    // ── IME composition ─────────────────────────────────────────────────────

    #[test]
    fn a_first_composition_keystroke_replaces_the_selection() {
        // No marked range and no replacement range: the composition lands on
        // whatever was selected.
        assert_eq!(marked_replace_range("abc", None, None, 1..3), 1..3);
    }

    #[test]
    fn a_replacement_range_is_absolute_while_no_composition_is_up() {
        assert_eq!(marked_replace_range("a€bc", Some(1..3), None, 0..0), 1..5);
    }

    #[test]
    fn a_replacement_range_is_relative_to_the_composition_once_one_is_up() {
        // "a€" is 4 bytes / 2 UTF-16 units, so a composition marked at bytes
        // 4..7 starts at UTF-16 offset 2. AppKit's 1..2 is relative to *that*,
        // meaning UTF-16 3..4 — bytes 5..6. Read as absolute it would be 1..2,
        // which isn't even inside the composition.
        let marked = 4..7; // the three bytes of "xyz" in "a€xyz"
        assert_eq!(
            marked_replace_range("a€xyz", Some(1..2), Some(marked), 0..0),
            5..6
        );
    }

    #[test]
    fn no_replacement_range_with_a_composition_up_replaces_all_of_it() {
        assert_eq!(
            marked_replace_range("a€xyz", None, Some(4..7), 0..0),
            4..7
        );
    }

    // ── preedit underline ───────────────────────────────────────────────────

    /// Glyphs for `text`, each mapping to its own source byte from `start` — the
    /// trivial (source-view) mapping, which is all these runs need.
    fn glyphs(text: &str, start: usize) -> Vec<Glyph> {
        text.char_indices()
            .map(|(i, ch)| Glyph {
                ch,
                style: CoreStyle::default(),
                src: start + i,
                stop: true,
            })
            .collect()
    }

    #[test]
    fn a_preedit_underlines_only_the_composed_glyphs() {
        let styler = test_run_style(gpui::font("Helvetica"));
        // "abcde", composition over the source bytes of "cd".
        let runs = build_runs(&glyphs("abcde", 0), &styler, Some(&(2..4)));
        // One unstyled run either side of the underlined middle — the marked
        // range has to split runs even though every glyph shares a style.
        let lens: Vec<usize> = runs.iter().map(|r| r.len).collect();
        assert_eq!(lens, vec![2, 2, 1]);
        assert!(runs[0].underline.is_none());
        assert!(runs[1].underline.is_some());
        assert!(runs[2].underline.is_none());
    }

    #[test]
    fn no_composition_leaves_one_unbroken_run() {
        let styler = test_run_style(gpui::font("Helvetica"));
        let runs = build_runs(&glyphs("abcde", 0), &styler, None);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len, 5);
        assert!(runs[0].underline.is_none());
    }

    // ── table column fitting ─────────────────────────────────────────────────

    use super::{BORDER, CELL_PAD_X, MIN_COL_PX, fit_widths_px};

    /// What the columns plus their chrome actually occupy.
    fn grid_width(widths: &[f32]) -> f32 {
        widths.iter().sum::<f32>() + widths.len() as f32 * (BORDER + 2.0 * CELL_PAD_X) + BORDER
    }

    #[test]
    fn columns_that_already_fit_are_left_alone() {
        let mut w = [100.0, 50.0];
        fit_widths_px(&mut w, 1000.0);
        assert_eq!(w, [100.0, 50.0], "a table that fits must not be stretched");
    }

    #[test]
    fn an_overwide_grid_is_taken_from_the_widest_column() {
        // The narrow column is already carrying its content; the loss belongs to
        // the one with room to give.
        let mut w = [400.0, 40.0];
        fit_widths_px(&mut w, 300.0);
        assert!(grid_width(&w) <= 300.0, "still overflows: {w:?}");
        assert_eq!(w[1], 40.0, "the narrow column should not have been touched");
    }

    #[test]
    fn shrinking_levels_the_widest_columns_rather_than_gutting_one() {
        let mut w = [400.0, 380.0, 30.0];
        fit_widths_px(&mut w, 400.0);
        assert!(grid_width(&w) <= 400.0, "still overflows: {w:?}");
        assert!(
            (w[0] - w[1]).abs() <= 1.0,
            "two equally greedy columns should end up level, got {w:?}"
        );
    }

    #[test]
    fn a_column_is_never_squeezed_below_the_floor() {
        // More columns than the surface can hold: every column bottoms out and
        // the grid overflows, which is the honest outcome — there is nothing left
        // to give, and shredding text one letter per line is worse.
        let mut w = [200.0; 8];
        fit_widths_px(&mut w, 100.0);
        assert!(
            w.iter().all(|&c| c >= MIN_COL_PX),
            "a column went below the floor: {w:?}"
        );
    }
}

/// The table grid, measured through a real text system.
///
/// Everything the widget draws goes through `shape_line`, and a `Window` is the
/// only way to reach one — so these drive gpui's test harness, which hands out a
/// real window with no platform surface on screen. Its text system gives each
/// character its own advance, which is what makes these tests worth running: the
/// bug being pinned is a *proportional* one, invisible under a monospace font.
#[cfg(test)]
mod table_layout_tests {
    use super::*;
    use gpui::TestAppContext;
    use gpui::VisualTestContext;
    use leaf_core::{Doc, View};

    /// `| Name | Qty |` with Name left-aligned and Qty right-aligned.
    const TABLE: &str = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n| Fig | 12 |\n";

    fn doc_with(name: &str, body: &str) -> Doc {
        // The fixture name doubles as the temp file's; the counter keeps two
        // tests picking the same one from reading each other's body under the
        // parallel runner.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_gpui_test_{name}_{seq}.md"));
        std::fs::write(&p, body).unwrap();
        let mut doc = Doc::open(p).unwrap();
        doc.view = View::Wysiwyg;
        doc.build_visual_unwrapped();
        doc
    }

    /// Lay `body`'s first table out at `avail` pixels wide, exactly as prepaint
    /// would, and hand back the rows and the geometry its chrome is drawn from.
    fn lay_out(
        cx: &mut TestAppContext,
        name: &str,
        body: &str,
        avail: f32,
    ) -> (Vec<RowLayout>, TableGeom, TableInfo) {
        let doc = doc_with(name, body);
        let info = doc.vmap.tables.first().expect("a table").clone();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| {
            let font = window.text_style().font();
            let mut shaper = Shaper {
                window,
                styler: test_run_style(font.clone()),
                body_size: px(16.0),
                heading_scale: EditorStyle::default().heading_scale,
                line_ratio: 1.5,
                prev: HashMap::new(),
                fresh: HashMap::new(),
                breaks: HashMap::new(),
                prev_breaks: HashMap::new(),
            };
            let mut rows = Vec::new();
            let geom = layout_table(&mut shaper, &info, avail, None, &mut rows)
                .expect("the table should lay out");
            (rows, geom, info)
        })
    }

    /// The x each segment of `row` is placed at.
    fn xs(row: &RowLayout) -> Vec<f32> {
        row.segments.iter().map(|s| f32::from(s.x)).collect()
    }

    #[gpui::test]
    fn a_columns_cells_all_start_at_the_same_x(cx: &mut TestAppContext) {
        // The whole point. leaf-core pads cells with spaces to a monospace column
        // count, so in a proportional font "Pear" and "Fig" push their row's `│`
        // to different x and the grid shears. Placing each cell at its column's
        // computed x is what fixes it — and a left-aligned column is the one
        // whose text starts flush at that x, so it's the honest thing to assert.
        let (rows, _, _) = lay_out(cx, "square", TABLE, 800.0);
        let col0: Vec<f32> = rows.iter().map(|r| xs(r)[0]).collect();
        assert!(
            col0.windows(2).all(|w| (w[0] - w[1]).abs() < 0.01),
            "column 1 shears across rows: {col0:?}"
        );
    }

    #[gpui::test]
    fn a_right_aligned_column_ends_flush(cx: &mut TestAppContext) {
        // "3" and "12" are different widths; right alignment means their *ends*
        // line up, not their starts. The delimiter row (`|----:|`) is the only
        // place that alignment is recorded, so this also proves it survived the
        // trip through leaf-core's structure.
        let (rows, _, _) = lay_out(cx, "align", TABLE, 800.0);
        let right_edge = |r: &RowLayout| {
            let s = &r.segments[1];
            f32::from(s.x + s.shaped.width)
        };
        let (pear, fig) = (right_edge(&rows[1]), right_edge(&rows[2]));
        assert!(
            (pear - fig).abs() < 0.01,
            "right-aligned cells should end flush: {pear} vs {fig}"
        );
        // And they must not *start* flush, or the alignment did nothing.
        assert!(
            (xs(&rows[1])[1] - xs(&rows[2])[1]).abs() > 0.01,
            "a right-aligned column whose cells start flush isn't aligned at all"
        );
    }

    /// Assert every segment's text sits inside the column it answers for. Uses
    /// each segment's own `field` rather than indexing `geom.bounds`, because a
    /// column that ran dry contributes no segment to a row — so a segment's
    /// position in the row is not its column number.
    fn assert_cells_stay_in_their_columns(rows: &[RowLayout]) {
        for (r, row) in rows.iter().enumerate() {
            for (i, seg) in row.segments.iter().enumerate() {
                let (lo, hi) = (f32::from(seg.field.0), f32::from(seg.field.1));
                let (x0, x1) = (f32::from(seg.x), f32::from(seg.x + seg.shaped.width));
                assert!(
                    x0 >= lo - 0.01 && x1 <= hi + 0.01,
                    "row {r} segment {i}: text {x0}..{x1} escapes its column {lo}..{hi}"
                );
            }
        }
    }

    #[gpui::test]
    fn every_cell_is_inside_its_own_column(cx: &mut TestAppContext) {
        // A cell that overhangs its column lands on the border or in the next
        // cell — the failure the box-drawn version had no way to prevent.
        let (rows, _, _) = lay_out(cx, "bounds", TABLE, 800.0);
        assert_cells_stay_in_their_columns(&rows);
    }

    #[gpui::test]
    fn a_click_in_a_cell_lands_in_that_cell(cx: &mut TestAppContext) {
        // The caret has to survive the trip into a cell laid out this way. Click
        // the middle, the left edge, and past the right edge of each cell of the
        // "Pear | 3" row: every one belongs to the cell clicked. This is the
        // round-trip the whole segment model exists to keep.
        let (rows, geom, info) = lay_out(cx, "click", TABLE, 800.0);
        let row = &rows[1]; // row 0 is the header; the borders aren't rows here
        for c in 0..2 {
            let cell = &info.grid[1].cells[c];
            let seg = &row.segments[c];
            let probes = [
                ("its left edge", seg.x),
                ("its middle", seg.x + seg.shaped.width / 2.0),
                // Past the text but still inside the column: the gutter, which
                // is where a click "after the last character" of a cell falls.
                ("its right gutter", px(geom.bounds[c + 1] - 1.0)),
            ];
            for (where_, x) in probes {
                let off = row.src_at_index(row.index_for_x(x));
                assert!(
                    off >= cell.start && off <= cell.end,
                    "a click at {where_} of column {c} landed at {off}, outside \
                     that cell's {}..{}",
                    cell.start,
                    cell.end
                );
            }
            // The left edge is the cell's own start, not merely inside it.
            assert_eq!(
                row.src_at_index(row.index_for_x(seg.x)),
                cell.start,
                "clicking a cell's left edge should land at its start"
            );
        }
    }

    #[gpui::test]
    fn down_from_a_cell_lands_in_the_cell_below_not_the_column_beside(
        cx: &mut TestAppContext,
    ) {
        // Vertical motion is index arithmetic over rows, and a table row is one
        // row however many cells it has — so Down from "Pear" must reach "Fig",
        // keeping its column. This is what the segment model buys: were each cell
        // its own row, Down from "Pear" would land on "3".
        let (rows, _, _) = lay_out(cx, "down", TABLE, 800.0);
        let from = &rows[1];
        let x = from.x_at(from.segments[0].first);
        let to = &rows[2];
        let off = to.src_at_index(to.index_for_x(x));
        assert_eq!(
            TABLE[off..].chars().next(),
            Some('F'),
            "Down from \"Pear\" should reach \"Fig\", landed at offset {off}"
        );
    }

    #[gpui::test]
    fn painting_a_code_block_does_not_panic(cx: &mut TestAppContext) {
        // Drives the real prepaint+paint over a fenced block: the box quad, the
        // content-mask clip, the language chip, and the caret-follow horizontal
        // scroll (caret parked inside a line far wider than the viewport). A
        // regression in any of those quad/shape calls panics here.
        let doc = doc_with(
            "paint_code",
            "text\n\n```rust\nlet x = 1;\nlet very_long_line_that_is_far_wider_than_the_narrow_test_viewport_here = 1;\n```\n\nafter\n",
        );
        let window = cx.add_window(|_, cx| Editor::new(cx, Some(doc)));
        let editor = window.root(cx).unwrap();
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        editor.update(&mut vcx, |e, _| {
            if let Some(d) = e.doc.as_mut() {
                d.caret = 60; // deep inside the long code line
            }
        });
        vcx.draw(
            gpui::point(px(0.0), px(0.0)),
            gpui::size(px(240.0), px(400.0)),
            |_, _| TextElement { editor: editor.clone() },
        );
    }

    /// Write a tiny solid-colour PNG to a uniquely-named temp file and return its
    /// path — a real on-disk image for the loader to decode.
    fn write_test_png(name: &str, w: u32, h: u32) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!("leaf_gpui_test_img_{name}_{seq}.png"));
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 40, 40, 255]));
        img.save_with_format(&path, image::ImageFormat::Png).unwrap();
        path
    }

    #[test]
    fn load_image_file_decodes_a_real_png_to_a_render_image() {
        // The whole decode path: read → guess format → decode → RGBA→BGRA swap →
        // RenderImage. A real PNG on disk exercises it end to end (short of the
        // GPU upload, which `painting_a_block_image_does_not_panic` drives).
        let path = write_test_png("decode", 6, 4);
        let img = load_image_file(&path).expect("the PNG should decode");
        let size = img.size(0);
        assert_eq!((size.width.0, size.height.0), (6, 4), "intrinsic size survives");
        // A bogus path fails cleanly rather than panicking.
        assert!(load_image_file(Path::new("/no/such/file.png")).is_none());
    }

    #[gpui::test]
    fn painting_a_block_image_does_not_panic(cx: &mut TestAppContext) {
        // Drives the real prepaint+paint over a block image: resolve → decode →
        // reserve the box row → `Window::paint_image` (which uploads the BGRA
        // frame to the sprite atlas). A regression in the decode, the sizing, or
        // the paint call panics here. A missing image and a remote URL exercise
        // the text-placeholder fallback in the same pass.
        let png = write_test_png("paint", 800, 400);
        let body = format!(
            "text\n\n![a photo]({})\n\nmiddle\n\n![gone](nope.png)\n\n![remote](https://x.dev/a.png)\n\nafter\n",
            png.display()
        );
        let doc = doc_with("paint_image", &body);
        let window = cx.add_window(|_, cx| Editor::new(cx, Some(doc)));
        let editor = window.root(cx).unwrap();
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.draw(
            gpui::point(px(0.0), px(0.0)),
            gpui::size(px(240.0), px(600.0)),
            |_, _| TextElement { editor: editor.clone() },
        );
    }

    #[test]
    fn gather_logical_tags_code_rows_and_carries_the_language() {
        // A fenced block's code lines are gathered as `Line`s tagged with their
        // block index (so prepaint can box and scroll them), prose as `None`, and
        // the block's language rides the map for the label.
        let doc = doc_with("code_gather", "text\n\n```rust\nlet x = 1;\n```\n\nafter\n");
        let logical = gather_logical(&doc);
        let code_lines: Vec<String> = logical
            .iter()
            .filter_map(|l| match l {
                Logical::Line { glyphs, code: Some(_), .. } => {
                    Some(glyphs.iter().map(|g| g.ch).collect())
                }
                _ => None,
            })
            .collect();
        assert_eq!(code_lines, vec!["let x = 1;".to_string()]);
        // Prose lines carry no code tag.
        assert!(logical.iter().any(|l| matches!(l, Logical::Line { code: None, .. })));
        assert_eq!(doc.vmap.code_blocks[0].lang.as_deref(), Some("rust"));
    }

    #[test]
    fn no_box_drawing_reaches_the_gui() {
        // The bug, stated as the user sees it: a table drawn as *text*. Every
        // border leaf-core spells with a box glyph has to be gone from what the
        // GUI shapes — if any of those rows leak through they paint the old
        // sheared picture underneath the real grid.
        let doc = doc_with("no_box", &format!("intro\n\n{TABLE}\noutro\n"));
        let logical = gather_logical(&doc);
        let mut tables = 0;
        for l in &logical {
            match l {
                Logical::Table(_) => tables += 1,
                Logical::Image { .. } => {}
                Logical::Line { glyphs, .. } => {
                    let text: String = glyphs.iter().map(|g| g.ch).collect();
                    assert!(
                        !text.contains(['┌', '┬', '┐', '├', '┼', '┤', '└', '┴', '┘', '│', '─']),
                        "box drawing leaked into a line the GUI will shape: {text:?}"
                    );
                }
            }
        }
        assert_eq!(tables, 1, "the table should have been gathered as a table");
        // And the prose around it still came through.
        let prose: Vec<String> = logical
            .iter()
            .filter_map(|l| match l {
                Logical::Line { glyphs, .. } => Some(glyphs.iter().map(|x| x.ch).collect::<String>()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(prose, ["intro", "outro"], "the prose around the table was lost");
    }

    #[test]
    fn a_block_image_is_gathered_as_an_image_not_a_line() {
        // A block image's placeholder row is pulled out as `Logical::Image`, its
        // box-picture skipped the way a table's is — so the GUI paints a raster
        // there instead of shaping the `🖼 alt` label as prose.
        let doc = doc_with("gather_img", "intro\n\n![a cat](cat.png)\n\noutro\n");
        let logical = gather_logical(&doc);
        let imgs: Vec<&ImageInfo> = logical
            .iter()
            .filter_map(|l| match l {
                Logical::Image { info, .. } => Some(info),
                _ => None,
            })
            .collect();
        assert_eq!(imgs.len(), 1, "the image should be gathered as an image");
        assert_eq!(imgs[0].destination, "cat.png");
        assert_eq!(imgs[0].alt, "a cat");
        // The prose around it still came through as lines.
        let prose: Vec<String> = logical
            .iter()
            .filter_map(|l| match l {
                Logical::Line { glyphs, .. } => Some(glyphs.iter().map(|x| x.ch).collect::<String>()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(prose, ["intro", "outro"]);
    }

    #[test]
    fn resolve_image_path_handles_local_relative_and_rejects_remote() {
        let dir = Path::new("/docs/notes");
        assert_eq!(
            resolve_image_path("pics/cat.png", Some(dir)),
            Some(PathBuf::from("/docs/notes/pics/cat.png")),
            "a relative path joins the document directory"
        );
        assert_eq!(
            resolve_image_path("/abs/cat.png", Some(dir)),
            Some(PathBuf::from("/abs/cat.png")),
            "an absolute path is taken as-is"
        );
        // Remote and data URIs, and a relative path with no doc dir, are not
        // synchronously loadable — they fall back to the text placeholder.
        assert_eq!(resolve_image_path("https://x.dev/a.png", Some(dir)), None);
        assert_eq!(resolve_image_path("data:image/png;base64,AAAA", Some(dir)), None);
        assert_eq!(resolve_image_path("cat.png", None), None);
    }

    #[test]
    fn image_box_size_fits_width_and_caps_height_without_upscaling() {
        let dp = |w: i32, h: i32| Size { width: DevicePixels(w), height: DevicePixels(h) };
        // Wider than the editor: scaled down to the width, aspect preserved.
        let s = image_box_size(dp(800, 400), 400.0);
        assert_eq!((f32::from(s.width), f32::from(s.height)), (400.0, 200.0));
        // Smaller than the editor: never upscaled.
        let s = image_box_size(dp(100, 50), 400.0);
        assert_eq!((f32::from(s.width), f32::from(s.height)), (100.0, 50.0));
        // Very tall: capped at IMAGE_MAX_H, aspect preserved.
        let s = image_box_size(dp(100, 2000), 400.0);
        assert_eq!(f32::from(s.height), IMAGE_MAX_H);
        assert!(f32::from(s.width) < 100.0);
    }

    #[test]
    fn the_source_view_still_shows_a_table_as_the_text_it_is() {
        // The grid is a WYSIWYG idea. In the source view a table is raw Markdown
        // — pipes and dashes the caret edits directly — and must not be caught by
        // the table path.
        let mut doc = doc_with("source_view", TABLE);
        doc.view = View::Source;
        let logical = gather_logical(&doc);
        assert!(
            logical.iter().all(|l| matches!(l, Logical::Line { .. })),
            "the source view must not build a grid"
        );
        let first: String = match &logical[0] {
            Logical::Line { glyphs, .. } => glyphs.iter().map(|x| x.ch).collect(),
            _ => unreachable!(),
        };
        assert_eq!(first, "| Name | Qty |", "the source view shows the source");
    }

    // ── the shape cache ──────────────────────────────────────────────────────
    //
    // `Rc::ptr_eq` is the whole reason these can be tests rather than claims: it
    // says whether the *same* shape came back, which is exactly what a hit and a
    // miss differ by and nothing else can see.

    fn glyphs_of(text: &str, start: usize, style: CoreStyle) -> Vec<Glyph> {
        text.char_indices()
            .map(|(i, ch)| Glyph { ch, style, src: start + i, stop: true })
            .collect()
    }

    /// Run `f` with a shaper over a real window.
    fn with_shaper<R>(cx: &mut TestAppContext, f: impl FnOnce(&mut Shaper) -> R) -> R {
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, _| {
            let font = window.text_style().font();
            let mut shaper = Shaper {
                window,
                styler: test_run_style(font.clone()),
                body_size: px(16.0),
                heading_scale: EditorStyle::default().heading_scale,
                line_ratio: 1.5,
                prev: HashMap::new(),
                fresh: HashMap::new(),
                breaks: HashMap::new(),
                prev_breaks: HashMap::new(),
            };
            f(&mut shaper)
        })
    }

    #[gpui::test]
    fn text_that_only_moved_is_not_reshaped(cx: &mut TestAppContext) {
        // The point of the whole design. An edit shifts every source offset
        // after it, so the *same paragraph* arrives with different `src` values
        // — and a cache that noticed would re-shape the entire document below
        // the caret on every keystroke. A shape doesn't depend on where its text
        // came from, so this must be a hit.
        with_shaper(cx, |sh| {
            let before = glyphs_of("the quick brown fox", 0, CoreStyle::default());
            let a = sh.shape(&before, None);
            // Same text, every offset moved along by one keystroke.
            let after = glyphs_of("the quick brown fox", 1, CoreStyle::default());
            let b = sh.shape(&after, None);
            assert!(
                Rc::ptr_eq(&a, &b),
                "text that only moved was re-shaped — typing at the top of a \
                 file would re-shape everything below it"
            );
        });
    }

    #[gpui::test]
    fn different_text_gets_its_own_shape(cx: &mut TestAppContext) {
        with_shaper(cx, |sh| {
            let a = sh.shape(&glyphs_of("alpha", 0, CoreStyle::default()), None);
            let b = sh.shape(&glyphs_of("beta", 0, CoreStyle::default()), None);
            assert!(!Rc::ptr_eq(&a, &b), "two different texts shared one shape");
            assert_eq!(a.text.as_ref(), "alpha");
            assert_eq!(b.text.as_ref(), "beta");
        });
    }

    #[gpui::test]
    fn the_same_text_in_a_different_style_is_shaped_again(cx: &mut TestAppContext) {
        // The style picks the run's font — bold text is a different shape, and
        // reusing the plain one would silently un-bold it.
        with_shaper(cx, |sh| {
            let plain = sh.shape(&glyphs_of("word", 0, CoreStyle::default()), None);
            let bold = sh.shape(&glyphs_of("word", 0, CoreStyle::default().bold()), None);
            assert!(!Rc::ptr_eq(&plain, &bold), "bold reused the plain shape");
        });
    }

    #[gpui::test]
    fn a_preedit_is_not_served_the_undecorated_shape(cx: &mut TestAppContext) {
        // The IME underlines the composed glyphs, which is part of the shape.
        with_shaper(cx, |sh| {
            let g = glyphs_of("word", 0, CoreStyle::default());
            let plain = sh.shape(&g, None);
            let composing = sh.shape(&g, Some(&(0..4)));
            assert!(
                !Rc::ptr_eq(&plain, &composing),
                "a preedit was served the shape without its underline"
            );
        });
    }

    #[gpui::test]
    fn shapes_no_longer_on_screen_are_dropped(cx: &mut TestAppContext) {
        // The cache tracks the document as it stands, not everything it has ever
        // been — else typing would grow it without bound for a session.
        with_shaper(cx, |sh| {
            sh.shape(&glyphs_of("gone", 0, CoreStyle::default()), None);
            // A new paint: last paint's shapes are the ones up for reuse.
            sh.prev = std::mem::take(&mut sh.fresh);
            sh.shape(&glyphs_of("kept", 0, CoreStyle::default()), None);
            assert_eq!(sh.fresh.len(), 1, "only this paint's shape should survive");
            assert_eq!(sh.prev.len(), 1, "the unused one is still up for eviction");
            // `prev` is what prepaint throws away — the editor only gets `fresh`.
        });
    }

    // ── the layout cache ─────────────────────────────────────────────────────

    #[gpui::test]
    fn opening_another_document_does_not_paint_the_last_one(cx: &mut TestAppContext) {
        // A revision counts edits *within* a document, so every freshly opened
        // one starts at zero. Two different files therefore produce the *same*
        // layout key — and a cache that trusted the key alone would open the
        // second file and paint the first. Nothing about the key can catch this;
        // only the swap can.
        let a = doc_with("swap_a", "alpha alpha alpha\n");
        let b = doc_with("swap_b", "beta beta beta\n");
        assert_eq!(a.revision(), b.revision(), "the premise: equal revisions");

        let editor = cx.new(|cx| Editor::new(cx, Some(a)));
        editor.update(cx, |e, _| {
            // Stand in for a paint having cached rows for document A.
            e.layout_key = Some(LayoutKey {
                revision: 0,
                width: 800f32.to_bits(),
                source_view: true,
                marked: None,
            });
            e.last_rows = Rc::new(vec![RowLayout::prose(
                Default::default(),
                vec![0],
                vec![0, 1],
                1,
                px(24.0),
            )]);
        });
        editor.update(cx, |e, cx| e.set_doc(b, cx));
        editor.read_with(cx, |e, _| {
            assert!(
                e.layout_key.is_none(),
                "swapping the document must drop the last one's rows"
            );
        });
    }

    #[gpui::test]
    fn restyling_reshapes_rather_than_repainting_the_old_font(cx: &mut TestAppContext) {
        // The font is shaped into the rows; the key can't see it.
        let editor = cx.new(|cx| Editor::new(cx, Some(doc_with("restyle", "text\n"))));
        editor.update(cx, |e, _| {
            e.layout_key = Some(LayoutKey {
                revision: 0,
                width: 800f32.to_bits(),
                source_view: true,
                marked: None,
            });
        });
        editor.update(cx, |e, cx| {
            let mut s = EditorStyle::default();
            s.font_family = "Courier".into();
            e.set_style(s, cx);
        });
        editor.read_with(cx, |e, _| {
            assert!(
                e.layout_key.is_none(),
                "a font change must re-shape, not reuse the old font's rows"
            );
        });
    }

    #[gpui::test]
    fn a_selected_cell_is_highlighted_only_as_far_as_its_own_text(cx: &mut TestAppContext) {
        // The right edge of a cell's selection quad is the end of *its* text. It
        // is tempting to ask the row where flat index `b` sits, but at a cell
        // boundary that index is already the next cell's first character — the
        // quad would run across the border and both gutters, highlighting a
        // neighbour that isn't selected.
        let (rows, _, _) = lay_out(cx, "sel", TABLE, 800.0);
        let row = &rows[1];
        let (seg0, seg1) = (&row.segments[0], &row.segments[1]);
        let b = seg1.first; // one past cell 0's last character

        let ends = row.x_in(0, b);
        assert!(
            (f32::from(ends) - f32::from(seg0.x + seg0.shaped.width)).abs() < 0.01,
            "a cell's selection should end at its own text ({:?}), got {ends:?}",
            seg0.x + seg0.shaped.width
        );
        assert!(
            ends < seg1.x,
            "the highlight reached into the next cell: {ends:?} >= {:?}",
            seg1.x
        );
    }

    #[gpui::test]
    fn a_table_in_a_blockquote_keeps_its_gutter(cx: &mut TestAppContext) {
        // A table nested in a quote has to render the quote's gutter and start
        // past it. Drawing the grid flush at the left margin would take the table
        // out of the quote it is plainly inside.
        let body = "> | a | b |\n> |---|---|\n> | c | d |\n";
        let (rows, geom, info) = lay_out(cx, "bq", body, 800.0);
        assert!(!info.prefix.is_empty(), "the quote's prefix should be carried");
        assert!(
            geom.bounds[0] > 0.0,
            "the grid should start past the gutter, not at {}",
            geom.bounds[0]
        );
        // Every grid row draws the gutter, and it opens the row.
        for (r, row) in rows.iter().enumerate() {
            let first = &row.segments[0];
            assert_eq!(
                f32::from(first.x),
                0.0,
                "row {r}'s gutter should open the row"
            );
            assert!(
                first.shaped.text.contains('│'),
                "row {r} lost the quote gutter: {:?}",
                first.shaped.text
            );
        }
    }

    #[gpui::test]
    fn a_wrapped_cell_still_fits_inside_its_column(cx: &mut TestAppContext) {
        // Cells wrap to their column, then a gutter space is appended to carry
        // the end stop. Wrapping to the full column width would let that space
        // push the line past it — a glyph landing on the border or in the next
        // cell, which is exactly what a column is supposed to prevent.
        let body = "| K | Notes |\n|---|-------|\n| a | a rather long note that has to wrap somewhere |\n";
        let (rows, geom, _) = lay_out(cx, "wrapfit", body, 240.0);
        assert!(
            geom.bands[1].0.len() > 1,
            "the note should have wrapped, else this proves nothing"
        );
        assert_cells_stay_in_their_columns(&rows);
    }

    #[gpui::test]
    fn the_chrome_closes_the_grid_it_was_measured_from(cx: &mut TestAppContext) {
        // The borders are real geometry now, so they can be wrong in ways box
        // glyphs never could: a rule at the wrong x is a line through a cell's
        // text. Every vertical must sit on a column boundary, and there must be
        // one per boundary — the outer two included, or the table has no box.
        let (rows, geom, _) = lay_out(cx, "chrome", TABLE, 800.0);
        let style = EditorStyle::default();
        let tops = row_tops(&rows);
        let (fills, borders) = table_chrome(&geom, px(0.0), px(0.0), &tops, &style);

        let verticals: Vec<f32> = borders
            .iter()
            .filter(|q| q.bounds.size.width <= px(BORDER))
            .map(|q| f32::from(q.bounds.origin.x))
            .collect();
        assert_eq!(
            verticals, geom.bounds,
            "every column boundary should carry a rule, and nothing else should"
        );

        // The head is filled and the single body-row pair leaves the first clear:
        // with a header plus two body rows, exactly one fill each.
        assert_eq!(fills.len(), 2, "expected a header fill and one stripe");
        assert_eq!(
            fills[0].background,
            style.table_header.into(),
            "the first fill should be the header's"
        );

        // The bottom rule stays inside the table rather than bleeding onto the
        // row below it.
        let bottom = borders
            .iter()
            .filter(|q| q.bounds.size.height <= px(BORDER))
            .map(|q| f32::from(q.bounds.origin.y))
            .fold(0.0f32, f32::max);
        let table_bottom = 24.0 * geom.rows.end as f32;
        assert!(
            bottom <= table_bottom - BORDER,
            "the bottom rule at {bottom} escapes the table's {table_bottom}"
        );
    }

    #[gpui::test]
    fn an_overwide_table_wraps_its_cells_rather_than_running_off(cx: &mut TestAppContext) {
        // Sized to content the grid would run past the surface, where no amount
        // of caret motion reaches it. It has to come back inside — and the text
        // has to still be there, wrapped, not clipped.
        let body = "| Name | Notes |\n|------|-------|\n| Pear | a rather long note that will not fit |\n";
        let (rows, geom, _) = lay_out(cx, "overwide", body, 260.0);
        assert!(
            *geom.bounds.last().unwrap() <= 260.0,
            "the grid still overflows: right edge {}",
            geom.bounds.last().unwrap()
        );
        assert!(
            geom.bands[1].0.len() > 1,
            "the long cell should have wrapped onto more than one line"
        );
        // Every character of the note survived the wrap.
        let text: String = rows[geom.bands[1].0.clone()]
            .iter()
            .flat_map(|r| r.segments.iter().map(|s| s.shaped.text.to_string()))
            .collect();
        for word in ["rather", "long", "note", "not", "fit"] {
            assert!(text.contains(word), "{word:?} was lost in the wrap: {text:?}");
        }
    }

    // ── typographic roles ────────────────────────────────────────────────────

    #[gpui::test]
    fn a_heading_row_is_taller_than_a_body_row(cx: &mut TestAppContext) {
        // The whole reason rows are laid out at their own heights: a heading's
        // larger font needs a taller box, and stacking it on a uniform grid
        // would overlap the line below. A body row is exactly the line height;
        // an h1 is bigger, an h6 (its scale is < 1) no smaller than the body.
        with_shaper(cx, |sh| {
            let body = glyphs_of("Title", 0, CoreStyle::default());
            let h1 = glyphs_of("Title", 0, CoreStyle::default().role(Role::Heading(1)));
            let h6 = glyphs_of("Title", 0, CoreStyle::default().role(Role::Heading(6)));
            assert_eq!(sh.row_height(&body), px(24.0), "a body row is the line height");
            assert!(
                sh.row_height(&h1) > sh.row_height(&body),
                "an h1 row must be taller than a body row: {:?} vs {:?}",
                sh.row_height(&h1),
                sh.row_height(&body),
            );
            assert!(
                sh.row_height(&h6) >= sh.row_height(&body),
                "an h6 row is never shorter than the body",
            );
        });
    }

    #[test]
    fn the_gui_styles_roles_by_family_color_and_weight() {
        // Core now emits only a role; the GUI supplies the whole look. This pins
        // the mapping down: family (mono for code), color (default text for
        // headings/code, a link color, decoration muted), weight (headings bold
        // from their role), and the mark background.
        let theme = EditorStyle::default();
        let styler = test_run_style(gpui::font("Helvetica"));
        let run = |role| build_runs(&glyphs_of("x", 0, CoreStyle::default().role(role)), &styler, None)[0].clone();

        // Code shapes in the mono family, in the default text color — mono is the
        // distinguisher, so it no longer needs the terminal's green.
        let code = run(Role::Code);
        assert_eq!(code.font.family, "Menlo", "code should shape in the mono family");
        assert_eq!(code.color, theme.text, "code reads in the default text color, not green");

        // A heading stays in the body family but is bold from its role and in the
        // default text color — size and weight distinguish it, not a hue.
        let heading = run(Role::Heading(1));
        assert_eq!(heading.font.family, "Helvetica", "a heading stays in the body family");
        assert_eq!(heading.font.weight, gpui::FontWeight::BOLD, "a heading is bold from its role");
        assert_eq!(heading.color, theme.text, "a heading renders in the default text color");

        // A link takes the link color and is underlined; a mark carries the
        // highlight background; decoration is muted.
        let link = run(Role::Link);
        assert_eq!(link.color, theme.link, "a link takes the link color");
        assert!(link.underline.is_some(), "a link is underlined");
        assert_eq!(run(Role::Mark).background_color, Some(theme.mark_background), "mark is highlighted");
        assert_eq!(run(Role::QuoteGutter).color, theme.muted, "decoration is muted");
    }

    #[test]
    fn row_tops_sum_heights_and_invert() {
        // `row_tops` stacks rows by their own heights, and `row_at_y` is its
        // inverse — the pair the caret, selection, and mouse ride now that a row
        // is no longer one fixed height.
        let rows = vec![
            RowLayout::prose(Default::default(), vec![], vec![0], 0, px(40.0)),
            RowLayout::prose(Default::default(), vec![], vec![0], 0, px(24.0)),
            RowLayout::prose(Default::default(), vec![], vec![0], 0, px(24.0)),
        ];
        let tops = row_tops(&rows);
        assert_eq!(tops, vec![px(0.0), px(40.0), px(64.0), px(88.0)]);
        // A y inside each band resolves to that band's row; the tall first row
        // owns everything up to 40, which a uniform grid would have split.
        assert_eq!(row_at_y(&tops, px(0.0)), 0);
        assert_eq!(row_at_y(&tops, px(39.0)), 0);
        assert_eq!(row_at_y(&tops, px(40.0)), 1);
        assert_eq!(row_at_y(&tops, px(63.0)), 1);
        assert_eq!(row_at_y(&tops, px(64.0)), 2);
        // Past the bottom clamps to the last real row, not the boundary entry.
        assert_eq!(row_at_y(&tops, px(999.0)), 2);
    }
}

