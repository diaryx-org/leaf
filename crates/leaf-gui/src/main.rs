//! leaf-gui — a caret-based rich-text GUI editor, built on twig via `leaf-core`
//! and rendered with gpui.
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

mod style;

use std::ops::Range;
use std::path::PathBuf;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, Font, GlobalElementId, InspectorElementId,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, Render, ScrollHandle, ShapedLine, Style, TextAlign, UTF16Selection,
    Window, WindowBounds, WindowOptions, actions, anchored, deferred, div, fill, point,
    prelude::*, px, relative, rgba, size,
};
use gpui_platform::application;
use leaf_core::style::Style as CoreStyle;
use leaf_core::{BlockKind, Doc, Glyph, InlineKind, View};

use crate::style::text_run;

actions!(
    leaf,
    [
        Backspace, Delete, Left, Right, Up, Down, SelectLeft, SelectRight, SelectUp, SelectDown,
        Home, End, SelectHome, SelectEnd, Newline, Indent, Save, Quit, ToggleBold, ToggleItalic,
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
    ]
);

/// The editor entity: a `leaf_core::Doc` plus gpui focus, and the last painted
/// layout cached so a mouse event can hit-test pixels back to a source offset.
struct Editor {
    focus_handle: FocusHandle,
    /// The open document, or `None` before a file has been chosen — in which
    /// case the editor shows a `+` button that opens a file picker.
    doc: Option<Doc>,
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
}

impl Editor {
    // ── keyboard actions → leaf-core Doc ops ────────────────────────────────
    // Every handler is a no-op until a document is open (the `+` button state).
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
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_up(false);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_down(false);
        self.scroll_caret_into_view();
        cx.notify();
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
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_up(true);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_down(true);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_home(false);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_end(false);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_home(true);
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.move_end(true);
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
        doc.insert("\n");
        self.scroll_caret_into_view();
        cx.notify();
    }
    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.insert("    ");
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
    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.as_mut() else { return };
        doc.save();
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
        doc.set_block(BlockKind::Heading(level));
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
        doc.insert(&text);
        self.scroll_caret_into_view();
        cx.notify();
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
        let (row, _) = doc.caret_pos();
        let caret_top = text_bounds.top() + lh * (row as f32);
        let caret_bottom = caret_top + lh;

        let mut offset = self.scroll_handle.offset();
        if caret_top < view.top() {
            offset.y += view.top() - caret_top;
        } else if caret_bottom > view.bottom() {
            offset.y -= caret_bottom - view.bottom();
        } else {
            return;
        }
        offset.y = offset.y.clamp(-self.scroll_handle.max_offset().y, px(0.0));
        self.scroll_handle.set_offset(offset);
    }

    // ── file picker ─────────────────────────────────────────────────────────
    /// Open the platform file picker (from the `+` button). twig supports
    /// markdown, djot, and HTML; the picker filters to those extensions. When a
    /// file is chosen we open it into `self.doc` and re-render as the editor.
    fn open_file_dialog(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |editor, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            editor
                .update(cx, |editor, cx| {
                    match Doc::open(path) {
                        Ok(doc) => editor.doc = Some(doc),
                        Err(e) => eprintln!("leaf-gui: {e}"),
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
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
            _ => {
                // Triple-click (or more): select the caret's whole source
                // line/paragraph. leaf-core has no single call for this, so
                // it's placing the caret then riding move_home/move_end the
                // same way a keyboard Home then Shift-End would.
                doc.place_caret(off, false);
                doc.move_home(false);
                doc.move_end(true);
            }
        }
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
    fn offset_from_utf16(&self, target: usize) -> usize {
        let Some(doc) = self.doc.as_ref() else {
            return 0;
        };
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in doc.source.chars() {
            if utf16 >= target {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }
    fn offset_to_utf16(&self, target: usize) -> usize {
        let Some(doc) = self.doc.as_ref() else {
            return 0;
        };
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in doc.source.chars() {
            if utf8 >= target {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
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
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.selection_range());
        if let Some(doc) = self.doc.as_mut() {
            doc.edit(range.start, range.end, new_text);
        }
        self.marked_range = None;
        self.scroll_caret_into_view();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Scaffold: commit composition text immediately (no visible preedit).
        self.replace_text_in_range(range_utf16, new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
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

/// Merge a WYSIWYG row's per-glyph styles into `TextRun`s (adjacent glyphs of
/// equal style become one run), then map each through `to_gpui`/`text_run`.
fn build_runs(glyphs: &[Glyph], base: &Font) -> Vec<gpui::TextRun> {
    let mut segs: Vec<(usize, CoreStyle)> = Vec::new();
    for g in glyphs {
        let bytes = g.ch.len_utf8();
        if let Some(last) = segs.last_mut()
            && last.1 == g.style
        {
            last.0 += bytes;
            continue;
        }
        segs.push((bytes, g.style));
    }
    segs.into_iter()
        .map(|(len, st)| text_run(len, st, base))
        .collect()
}

/// Approximate how many character-columns fit in `width_px` — the wrap width
/// `leaf-core`'s `VisualMap` counts in. It wraps by character count (a monospace
/// assumption), so we estimate a character as ~0.62em of the current font.
fn columns(width_px: f32, font_size_px: f32) -> usize {
    ((width_px / (font_size_px * 0.62).max(1.0)) as usize).max(8)
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
        // Row count differs by view; for WYSIWYG we must build the map to know
        // it. Use last frame's width (or a sane default before the first paint).
        let font_size = f32::from(window.text_style().font_size.to_pixels(window.rem_size()));
        let last_w = self
            .editor
            .read(cx)
            .last_bounds
            .map(|b| f32::from(b.size.width))
            .unwrap_or(760.0);
        let cols = columns(last_w, font_size);
        let n = self.editor.update(cx, |e, _| {
            let Some(doc) = e.doc.as_mut() else { return 1 };
            match doc.view {
                View::Source => doc.source.split('\n').count().max(1),
                View::Wysiwyg => {
                    doc.build_visual(cols);
                    doc.vmap.num_rows().max(1)
                }
            }
        });
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (window.line_height() * n as f32).into();
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
        let cols = columns(f32::from(bounds.size.width), f32::from(font_size));

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
                .update(cx, |e, _| e.doc.as_mut().unwrap().build_visual(cols));
        }

        let editor = self.editor.read(cx);
        let doc = editor.doc.as_ref().unwrap();
        let sel = doc.selection();
        let (caret_row, caret_col) = doc.caret_pos();

        // Build a RowLayout per visual row of the active view.
        let shape_row = |text: String, char_srcs: Vec<usize>, char_byte: Vec<usize>,
                             runs: Vec<gpui::TextRun>, end_src: usize| {
            let shaped = window
                .text_system()
                .shape_line(text.into(), font_size, &runs, None);
            RowLayout { shaped, char_srcs, char_byte, end_src }
        };
        let mut rows: Vec<RowLayout> = Vec::new();
        match view {
            View::Source => {
                let mut start = 0usize;
                for line in doc.source.split('\n') {
                    let mut char_srcs = Vec::new();
                    let mut char_byte = vec![0usize];
                    let mut acc = 0usize;
                    for (i, ch) in line.char_indices() {
                        char_srcs.push(start + i);
                        acc += ch.len_utf8();
                        char_byte.push(acc);
                    }
                    let runs = if line.is_empty() {
                        Vec::new()
                    } else {
                        vec![text_run(line.len(), CoreStyle::default(), &font)]
                    };
                    rows.push(shape_row(line.to_string(), char_srcs, char_byte, runs, start + line.len()));
                    start += line.len() + 1;
                }
            }
            View::Wysiwyg => {
                for vrow in &doc.vmap.rows {
                    let mut text = String::new();
                    let mut char_srcs = Vec::new();
                    let mut char_byte = vec![0usize];
                    let mut acc = 0usize;
                    for g in &vrow.glyphs {
                        text.push(g.ch);
                        char_srcs.push(g.src);
                        acc += g.ch.len_utf8();
                        char_byte.push(acc);
                    }
                    let runs = build_runs(&vrow.glyphs, &font);
                    rows.push(shape_row(text, char_srcs, char_byte, runs, vrow.end_src));
                }
            }
        }
        if rows.is_empty() {
            rows.push(shape_row(String::new(), Vec::new(), vec![0], Vec::new(), 0));
        }

        let left = bounds.left();
        let top = bounds.top();
        let row_top = |row: usize| top + line_height * (row as f32);

        // Caret: caret_pos() gives (row, col) in the active view's grid; col is a
        // character index, which char_byte turns into an x within the row.
        let cr = caret_row.min(rows.len() - 1);
        let cc = caret_col.min(rows[cr].char_byte.len() - 1);
        let caret_x = rows[cr].shaped.x_for_index(rows[cr].char_byte[cc]);
        let cursor = if sel.is_none() {
            Some(fill(
                Bounds::new(point(left + caret_x, row_top(cr)), size(px(2.0), line_height)),
                gpui::blue(),
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
                        rgba(0x3311ff30),
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

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        // Cache the layout so mouse handlers can hit-test back to source offsets.
        let rows = std::mem::take(&mut prepaint.rows);
        self.editor.update(cx, |editor, _| {
            editor.last_rows = rows;
            editor.last_line_height = lh;
            editor.last_bounds = Some(bounds);
        });
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // No document open: an empty canvas with a `+` button that opens the
        // file picker (markdown / djot / HTML — the formats twig supports).
        let Some(doc) = self.doc.as_ref() else {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .size_full()
                .font_family("Helvetica")
                .bg(gpui::white())
                .text_color(gpui::rgb(0x1e1e1e))
                .key_context("Editor")
                .track_focus(&self.focus_handle(cx))
                .child(
                    div()
                        .id("open-file")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(96.0))
                        .rounded_full()
                        .bg(gpui::rgb(0xf0f0f0))
                        .text_color(gpui::rgb(0x555555))
                        .text_size(px(56.0))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|s| s.bg(gpui::rgb(0xe4e4e4)))
                        .child("+")
                        .on_click(cx.listener(|editor, _, window, cx| {
                            editor.open_file_dialog(window, cx)
                        })),
                )
                .child(
                    div()
                        .mt_4()
                        .text_color(gpui::rgb(0x999999))
                        .child("Open a markdown, djot, or HTML file"),
                );
        };
        let name = doc.file_name();
        let view = doc.view_name();
        let dirty = if doc.dirty { " ●" } else { "" };

        div()
            .flex()
            .flex_col()
            .size_full()
            .font_family("Helvetica")
            .bg(gpui::white())
            .text_color(gpui::rgb(0x1e1e1e))
            .key_context("Editor")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::left))
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
            .on_action(cx.listener(Self::toggle_bold))
            .on_action(cx.listener(Self::toggle_italic))
            .on_action(cx.listener(Self::toggle_view))
            .on_action(cx.listener(Self::save))
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
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .when_some(self.context_menu, |el, pos| {
                el.child(Self::render_context_menu(pos, &mut *cx))
            })
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .bg(gpui::rgb(0xf0f0f0))
                    .text_color(gpui::rgb(0x555555))
                    .child(format!(
                        "leaf-gui — {name}{dirty}   [{view}]   ⌘e view · ⌘b/⌘i/⌘⇧c/⌘⇧m bold/italic/code/mark · \
                         ⌃0-6 ¶/heading · ⌥←/→ word, ⌥⌫/⌦ delete · ⌘a select all · ⌘c/⌘x/⌘v copy/cut/paste · ⌘s save"
                    )),
            )
            .child(
                div()
                    .id("body")
                    .flex_1()
                    .p_3()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .text_size(px(16.0))
                    .line_height(px(24.0))
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

fn main() {
    // `leaf-gui <file> [wysiwyg|source]` — the optional second arg picks the
    // starting view (handy for screenshotting the WYSIWYG view without keys).
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The file argument is optional: with no file we open to an empty canvas
    // whose `+` button lets the user pick one. With a file, we open it directly.
    let doc: Option<Doc> = match args.first() {
        None => None,
        Some(path) => {
            let start_wysiwyg = args.get(1).map(|s| s == "wysiwyg").unwrap_or(false);
            match Doc::open(PathBuf::from(path)) {
                Ok(mut d) => {
                    if start_wysiwyg {
                        d.view = View::Wysiwyg;
                    }
                    Some(d)
                }
                Err(e) => {
                    eprintln!("leaf-gui: {e}");
                    std::process::exit(1);
                }
            }
        }
    };

    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("up", Up, None),
            KeyBinding::new("down", Down, None),
            KeyBinding::new("shift-left", SelectLeft, None),
            KeyBinding::new("shift-right", SelectRight, None),
            KeyBinding::new("shift-up", SelectUp, None),
            KeyBinding::new("shift-down", SelectDown, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("shift-home", SelectHome, None),
            KeyBinding::new("shift-end", SelectEnd, None),
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", Delete, None),
            KeyBinding::new("enter", Newline, None),
            KeyBinding::new("tab", Indent, None),
            KeyBinding::new("cmd-b", ToggleBold, None),
            KeyBinding::new("cmd-i", ToggleItalic, None),
            KeyBinding::new("cmd-e", ToggleView, None),
            KeyBinding::new("cmd-s", Save, None),
            KeyBinding::new("cmd-q", Quit, None),
            // Word motion / deletion — the macOS convention, mirroring
            // leaf-core's move_word_left/right and delete_word_back/forward.
            KeyBinding::new("alt-left", MoveWordLeft, None),
            KeyBinding::new("alt-right", MoveWordRight, None),
            KeyBinding::new("shift-alt-left", SelectWordLeft, None),
            KeyBinding::new("shift-alt-right", SelectWordRight, None),
            KeyBinding::new("alt-backspace", DeleteWordBack, None),
            KeyBinding::new("alt-delete", DeleteWordForward, None),
            // Select-all.
            KeyBinding::new("cmd-a", SelectAll, None),
            // Format parity with the TUI's ⌥ toolbar (⌥c code, ⌥m mark, ⌥0-6
            // block). ⌥ is already spoken for by word motion above, so these
            // ride ⌘⇧ (code/mark) and ⌃ (block kind) instead — neither collides
            // with cmd-b/i/e/s/q or with each other.
            KeyBinding::new("cmd-shift-c", ToggleCode, None),
            KeyBinding::new("cmd-shift-m", ToggleMark, None),
            KeyBinding::new("ctrl-0", Paragraph, None),
            KeyBinding::new("ctrl-1", Heading1, None),
            KeyBinding::new("ctrl-2", Heading2, None),
            KeyBinding::new("ctrl-3", Heading3, None),
            KeyBinding::new("ctrl-4", Heading4, None),
            KeyBinding::new("ctrl-5", Heading5, None),
            KeyBinding::new("ctrl-6", Heading6, None),
            // Clipboard.
            KeyBinding::new("cmd-c", Copy, None),
            KeyBinding::new("cmd-x", Cut, None),
            KeyBinding::new("cmd-v", Paste, None),
        ]);
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());

        let bounds = Bounds::centered(None, size(px(820.0), px(640.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| Editor {
                        focus_handle: cx.focus_handle(),
                        doc,
                        marked_range: None,
                        is_selecting: false,
                        context_menu: None,
                        scroll_handle: ScrollHandle::new(),
                        last_rows: Vec::new(),
                        last_line_height: px(24.0),
                        last_bounds: None,
                    })
                },
            )
            .unwrap();

        window
            .update(cx, |editor, window, cx| {
                window.focus(&editor.focus_handle(cx), cx);
                cx.activate(true);
            })
            .unwrap();
    });
}
