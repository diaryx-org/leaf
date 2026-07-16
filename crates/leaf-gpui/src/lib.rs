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

use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, KeyBinding, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ScrollHandle,
    SharedString, ShapedLine, Style, Task, TextAlign, UTF16Selection, UnderlineStyle, Window,
    actions, anchored, deferred, div, fill, point, prelude::*, px, relative, rgb, rgba, size,
};
use leaf_core::style::Style as CoreStyle;
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
    /// Body font family.
    pub font_family: SharedString,
    /// Body font size and line height.
    pub font_size: Pixels,
    pub line_height: Pixels,
}

impl Default for EditorStyle {
    fn default() -> Self {
        EditorStyle {
            background: gpui::white(),
            text: rgb(0x1e1e1e).into(),
            caret: gpui::blue(),
            selection: rgba(0x3311ff30).into(),
            font_family: "Helvetica".into(),
            font_size: px(16.0),
            line_height: px(24.0),
        }
    }
}
use leaf_core::{BlockKind, DiskState, Doc, Glyph, InlineKind, InlineMarks, View};

use crate::style::text_run;

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
        // Clipboard (⌘C/⌘X/⌘V), backed by gpui's own clipboard — no external crate.
        Copy, Cut, Paste,
        // History (⌘Z / ⇧⌘Z).
        Undo, Redo,
        // Document start/end (⌘↑ / ⌘↓) and page motion, with ⇧ selecting.
        DocStart, DocEnd, SelectDocStart, SelectDocEnd,
        PageUp, PageDown, SelectPageUp, SelectPageDown,
        // Blockquote / list containers (⌘⇧9/8/7) and the link prompt (⌘K) —
        // the toolbar's remaining format commands, mirroring the TUI's set.
        ToggleBlockquote, ToggleBulletList, ToggleOrderedList, InsertLink,
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
    last_rows: Vec<RowLayout>,
    last_line_height: Pixels,
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
            last_rows: Vec::new(),
            last_line_height: px(24.0),
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
        cx.notify();
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
            _ => rows[r].shaped.x_for_index(rows[r].char_byte[gi]),
        };
        let tr = ((r as i32 + dir).max(0) as usize).min(rows.len() - 1);
        let byte = rows[tr].shaped.closest_index_for_x(x);
        Some((src_at(&rows[tr], byte), x))
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
    // ── clipboard (gpui's own, not an external crate) ───────────────────────
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_ref() else { return };
        if let Some(text) = doc.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        }
    }
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        let Some(text) = doc.selected_text().map(str::to_string) else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        doc.backspace();
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
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
        let lh = self.last_line_height;
        // The caret's *visual* row — located against the painted rows, since the
        // pixel wrap means one paragraph can span several rows (caret_pos() would
        // give the paragraph index instead).
        let row = if self.last_rows.is_empty() {
            0
        } else {
            locate_caret(&self.last_rows, doc.caret).0
        };
        let caret_top = text_bounds.top() + lh * (row as f32);
        let caret_bottom = caret_top + lh;

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
        let lh = f32::from(self.last_line_height).max(1.0);
        let rel_y = f32::from(pos.y - bounds.top()).max(0.0);
        let r = ((rel_y / lh) as usize).min(self.last_rows.len() - 1);
        let row = &self.last_rows[r];
        let byte = row.shaped.closest_index_for_x(pos.x - bounds.left());
        let ci = row
            .char_byte
            .iter()
            .position(|&b| b == byte)
            .unwrap_or(row.char_srcs.len());
        row.char_srcs.get(ci).copied().unwrap_or(row.end_src)
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
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = match (range_utf16, self.marked_range.clone()) {
            // Committing a composition (the IME's `insertText:` after a run of
            // `setMarkedText:`). macOS reports this replacement range relative
            // to the marked region, so it isn't an offset into this document at
            // all — and the region it means is exactly the composition we're
            // replacing. Taking it as absolute splices the finished word over
            // whatever happens to live at that offset instead.
            (Some(_), Some(marked)) | (None, Some(marked)) => marked,
            // No composition: an absolute range, from something like the
            // Accessibility Keyboard's word completion.
            (Some(r), None) => self.range_from_utf16(&r),
            (None, None) => self.selection_range(),
        };
        if let Some(doc) = self.doc.as_mut() {
            doc.edit(range.start, range.end, new_text);
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
            .edit(range.start, range.end, new_text);

        // An empty composition is the IME withdrawing it (⎋ out of a candidate
        // window): the text is gone, so there's nothing left to mark.
        self.marked_range = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());

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
        let row = &self.last_rows[r];
        let gi = gi.min(row.char_byte.len() - 1);
        let x = row.shaped.x_for_index(row.char_byte[gi]);
        let lh = self.last_line_height;
        Some(Bounds::new(
            element_bounds.origin + point(x, lh * (r as f32)),
            size(px(2.0), lh),
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

/// One painted row: a shaped line plus the mapping the caret/selection/mouse
/// code rides on. `char_srcs[i]` is the source byte offset the i-th character
/// came from; `char_byte[i]` is that character's byte offset *within the row's
/// own text* (so `char_byte` has `chars + 1` entries — the last is the row end).
/// `end_src` is the source offset the caret lands on past the last character.
///
/// For a source line these are trivial (each char maps to its own source byte);
/// for a WYSIWYG row they carry `Glyph::src`, so a hidden delimiter simply has
/// no character here and the caret steps over it.
struct RowLayout {
    shaped: ShapedLine,
    char_srcs: Vec<usize>,
    char_byte: Vec<usize>,
    end_src: usize,
}

/// The document body element: builds a [`RowLayout`] per visual row of the
/// active view, paints them with the caret and selection, and installs the
/// input handler.
struct TextElement {
    editor: Entity<Editor>,
}

struct Prepaint {
    rows: Vec<RowLayout>,
    line_height: Pixels,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

/// Merge a row's per-glyph styles into `TextRun`s (adjacent glyphs of equal
/// style become one run), then map each through `to_gpui`/`text_run`.
///
/// `marked` is the IME's live composition, which underlines the glyphs whose
/// *source* byte falls inside it — so it segments runs alongside the style, and
/// rides the glyphs' `src` exactly like everything else here (a preedit in
/// WYSIWYG is underlined across the visible text, hidden delimiters and all).
fn build_runs(glyphs: &[Glyph], base: &Font, marked: Option<&Range<usize>>) -> Vec<gpui::TextRun> {
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
            let mut run = text_run(len, st, base);
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
    window: &mut Window,
    font: &Font,
    font_size: Pixels,
    glyphs: &[Glyph],
    logical_end_src: usize,
    wrap_px: f32,
    marked: Option<&Range<usize>>,
    out: &mut Vec<RowLayout>,
) {
    if glyphs.is_empty() {
        let shaped = window.text_system().shape_line("".into(), font_size, &[], None);
        out.push(RowLayout {
            shaped,
            char_srcs: Vec::new(),
            char_byte: vec![0],
            end_src: logical_end_src,
        });
        return;
    }

    // Shape the whole line once, purely to measure where the breaks fall.
    let (text, char_byte) = row_text(glyphs);
    let runs = build_runs(glyphs, font, marked);
    let full = window
        .text_system()
        .shape_line(text.into(), font_size, &runs, None);
    let x = |byte: usize| f32::from(full.x_for_index(byte));

    // Greedy word wrap: walk maximal non-space runs, breaking before a word when
    // the line up to that word's end would overflow.
    let n = glyphs.len();
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

    // The common case — the line fits — reuses the shaped line we already have.
    if starts.len() == 1 {
        out.push(RowLayout {
            shaped: full,
            char_srcs: glyphs.iter().map(|g| g.src).collect(),
            char_byte,
            end_src: logical_end_src,
        });
        return;
    }

    for k in 0..starts.len() {
        let gs = starts[k];
        let ge = starts.get(k + 1).copied().unwrap_or(n);
        let sub = &glyphs[gs..ge];
        let (stext, scb) = row_text(sub);
        let sruns = build_runs(sub, font, marked);
        let shaped = window
            .text_system()
            .shape_line(stext.into(), font_size, &sruns, None);
        // The offset the caret lands on past this row: the block's end on the
        // last row, else the start of the next row's first glyph.
        let end_src = if ge == n {
            logical_end_src
        } else {
            let last = &glyphs[ge - 1];
            last.src + last.ch.len_utf8()
        };
        out.push(RowLayout {
            shaped,
            char_srcs: sub.iter().map(|g| g.src).collect(),
            char_byte: scb,
            end_src,
        });
    }
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
fn row_text(glyphs: &[Glyph]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut char_byte = vec![0usize];
    let mut acc = 0usize;
    for g in glyphs {
        text.push(g.ch);
        acc += g.ch.len_utf8();
        char_byte.push(acc);
    }
    (text, char_byte)
}

/// The source byte offset for a byte index within a row's text (the inverse of
/// `char_byte`): the glyph starting at `byte`, or the row's end past the last.
fn src_at(row: &RowLayout, byte: usize) -> usize {
    let ci = row
        .char_byte
        .iter()
        .position(|&b| b == byte)
        .unwrap_or(row.char_srcs.len());
    row.char_srcs.get(ci).copied().unwrap_or(row.end_src)
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
/// row's glyph count, meaning "just past the last glyph". At a soft-wrap boundary
/// one offset is both the end of a row and the start of the next; we bias to the
/// next row's start so the caret rides the wrapped line rather than its far edge.
fn locate_caret_core(row_srcs: &[&[usize]], row_end: &[usize], caret: usize) -> (usize, usize) {
    let n = row_srcs.len();
    for r in 0..n {
        let srcs = row_srcs[r];
        match srcs.iter().position(|&s| s >= caret) {
            Some(gi) => return (r, gi),
            None => {
                if caret <= row_end[r] {
                    if caret == row_end[r]
                        && r + 1 < n
                        && row_srcs[r + 1].first() == Some(&caret)
                    {
                        continue; // soft-wrap boundary → next row's start
                    }
                    return (r, srcs.len());
                }
            }
        }
    }
    let r = n.saturating_sub(1);
    (r, row_srcs.get(r).map(|s| s.len()).unwrap_or(0))
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
        // when the document itself is shorter than the screen.
        style.size.height = (window.line_height() * n as f32 + bottom_inset).into();
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
            let shaped = window
                .text_system()
                .shape_line("".into(), font_size, &[], None);
            return Prepaint {
                rows: vec![RowLayout {
                    shaped,
                    char_srcs: Vec::new(),
                    char_byte: vec![0],
                    end_src: 0,
                }],
                line_height,
                cursor: None,
                selections: Vec::new(),
            };
        }

        // The WYSIWYG map must be current before we read caret/selection, since
        // both ride it. Rebuild at the real width now that we have bounds.
        let view = self.editor.read(cx).doc.as_ref().unwrap().view;
        if view == View::Wysiwyg {
            self.editor
                .update(cx, |e, _| e.doc.as_mut().unwrap().build_visual_unwrapped());
        }

        // Gather the logical lines (glyphs owned) so we can shape them below with
        // a mutable window borrow after the document borrow is dropped. A logical
        // line is a whole paragraph (WYSIWYG) or a source line (Source); the pixel
        // wrap that follows turns each into one or more visual rows.
        let (logical_lines, sel, caret, caret_color, selection_color, marked): (
            Vec<(Vec<Glyph>, usize)>,
            _,
            usize,
            Hsla,
            Hsla,
            Option<Range<usize>>,
        ) = {
            let editor = self.editor.read(cx);
            let caret_color = editor.style.caret;
            let selection_color = editor.style.selection;
            let marked = editor.marked_range.clone();
            let doc = editor.doc.as_ref().unwrap();
            let sel = doc.selection();
            let caret = doc.caret;
            let mut lines: Vec<(Vec<Glyph>, usize)> = Vec::new();
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
                        lines.push((glyphs, start + line.len()));
                        start += line.len() + 1;
                    }
                }
                View::Wysiwyg => {
                    for vrow in &doc.vmap.rows {
                        lines.push((vrow.glyphs.clone(), vrow.end_src));
                    }
                }
            }
            (lines, sel, caret, caret_color, selection_color, marked)
        };

        // Wrap each logical line at the real pixel width.
        let mut rows: Vec<RowLayout> = Vec::new();
        for (glyphs, end_src) in &logical_lines {
            wrap_logical(
                window,
                &font,
                font_size,
                glyphs,
                *end_src,
                wrap_px,
                marked.as_ref(),
                &mut rows,
            );
        }
        if rows.is_empty() {
            let shaped = window.text_system().shape_line("".into(), font_size, &[], None);
            rows.push(RowLayout {
                shaped,
                char_srcs: Vec::new(),
                char_byte: vec![0],
                end_src: 0,
            });
        }

        let left = bounds.left();
        let top = bounds.top();
        let row_top = |row: usize| top + line_height * (row as f32);

        // Caret: locate its visual row/column from the source offset against the
        // painted rows — the pixel wrap means caret_pos()'s paragraph grid no
        // longer matches the rows on screen.
        let (cr, cgi) = locate_caret(&rows, caret);
        let cr = cr.min(rows.len() - 1);
        let cgi = cgi.min(rows[cr].char_byte.len() - 1);
        let caret_x = rows[cr].shaped.x_for_index(rows[cr].char_byte[cgi]);
        let cursor = if sel.is_none() {
            Some(fill(
                Bounds::new(point(left + caret_x, row_top(cr)), size(px(2.0), line_height)),
                caret_color,
            ))
        } else {
            None
        };

        // Selection: highlight, per row, the run of characters whose source byte
        // falls in the selection — visible-space, so hidden delimiters are skipped.
        let mut selections = Vec::new();
        if let Some((s0, s1)) = sel {
            for (r, row) in rows.iter().enumerate() {
                let mut a: Option<usize> = None;
                let mut b = 0usize;
                for (i, &src) in row.char_srcs.iter().enumerate() {
                    if src >= s0 && src < s1 {
                        a.get_or_insert(i);
                        b = i + 1;
                    }
                }
                if let Some(a) = a {
                    let x0 = row.shaped.x_for_index(row.char_byte[a]);
                    let x1 = row.shaped.x_for_index(row.char_byte[b]);
                    selections.push(fill(
                        Bounds::from_corners(
                            point(left + x0, row_top(r)),
                            point(left + x1, row_top(r) + line_height),
                        ),
                        selection_color,
                    ));
                }
            }
        }

        Prepaint {
            rows,
            line_height,
            cursor,
            selections,
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

        for quad in prepaint.selections.drain(..) {
            window.paint_quad(quad);
        }

        let lh = prepaint.line_height;
        let left = bounds.left();
        let top = bounds.top();
        for (r, row) in prepaint.rows.iter().enumerate() {
            let origin = point(left, top + lh * (r as f32));
            row.shaped
                .paint(origin, lh, TextAlign::Left, None, window, cx)
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

        // Cache the layout so mouse handlers can hit-test back to source offsets.
        let rows = std::mem::take(&mut prepaint.rows);
        self.editor.update(cx, |editor, _| {
            editor.last_row_count = rows.len();
            editor.last_rows = rows;
            editor.last_line_height = lh;
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
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::cut))
                    .on_action(cx.listener(Self::paste))
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


#[cfg(test)]
mod tests {
    use super::{
        Glyph, build_runs, locate_caret_core, marked_replace_range, utf16_to_utf8, utf8_to_utf16,
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
        let font = gpui::font("Helvetica");
        // "abcde", composition over the source bytes of "cd".
        let runs = build_runs(&glyphs("abcde", 0), &font, Some(&(2..4)));
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
        let font = gpui::font("Helvetica");
        let runs = build_runs(&glyphs("abcde", 0), &font, None);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len, 5);
        assert!(runs[0].underline.is_none());
    }
}
