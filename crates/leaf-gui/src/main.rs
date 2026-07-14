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
        // History (⌘Z / ⇧⌘Z).
        Undo, Redo,
        // Document start/end (⌘↑ / ⌘↓) and page motion, with ⇧ selecting.
        DocStart, DocEnd, SelectDocStart, SelectDocEnd,
        PageUp, PageDown, SelectPageUp, SelectPageDown,
        // Cancel a pending quit confirmation.
        Cancel,
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
    /// Visual-row count from the last paint — request_layout reserves height for
    /// it, since the true (pixel-wrapped) count is only known once we've laid out.
    last_row_count: usize,
    /// The "sticky" x the caret aims for through a run of vertical moves, and the
    /// caret offset it was computed for. If the caret has since moved by any other
    /// path (typing, a horizontal key, a click), `goal_caret` no longer matches and
    /// the goal is recomputed — so we never sprinkle resets across every handler.
    goal_x: Option<Pixels>,
    goal_caret: usize,
    /// Armed by a ⌘Q on a modified document: the first press warns (in the
    /// header), a second confirms. Cancelled by ⌘S or Escape.
    quit_armed: bool,
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
        self.quit_armed = false;
        cx.notify();
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
    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.doc.as_ref().is_some_and(|d| d.dirty);
        if !dirty || self.quit_armed {
            cx.quit();
        } else {
            self.quit_armed = true; // warn once; a second ⌘Q confirms
            cx.notify();
        }
    }
    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.quit_armed {
            self.quit_armed = false;
            cx.notify();
        }
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
            // Triple-click (or more): select the whole enclosing paragraph. This
            // reads the block's span from the AST, so it selects the entire
            // logical paragraph even when it soft-wraps across several rows.
            _ => doc.select_block_at(off),
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
        self.quit_armed = false; // typing means "not quitting after all"
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
    let runs = build_runs(glyphs, font);
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
        let sruns = build_runs(sub, font);
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
        let n = self.editor.read(cx).last_row_count.max(1);
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
        let (logical_lines, sel, caret): (Vec<(Vec<Glyph>, usize)>, _, usize) = {
            let editor = self.editor.read(cx);
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
            (lines, sel, caret)
        };

        // Wrap each logical line at the real pixel width.
        let mut rows: Vec<RowLayout> = Vec::new();
        for (glyphs, end_src) in &logical_lines {
            wrap_logical(window, &font, font_size, glyphs, *end_src, wrap_px, &mut rows);
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
            editor.last_row_count = rows.len();
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
                .on_action(cx.listener(Self::quit))
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
        let header = if self.quit_armed {
            "Unsaved changes — ⌘Q again to quit without saving, ⌘S to save, Esc to cancel".to_string()
        } else {
            format!(
                "leaf-gui — {name}{dirty}   [{view}]   ⌘e view · ⌘b/⌘i/⌘⇧c/⌘⇧m bold/italic/code/mark · \
                 ⌃0-6 ¶/heading · ⌥←/→ word, ⌥⌫/⌦ delete · ⌘↑/↓ doc ends · ⌘z/⇧⌘z undo · ⌘s save"
            )
        };

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
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::cancel))
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
                    .bg(if self.quit_armed { gpui::rgb(0xffe9b0) } else { gpui::rgb(0xf0f0f0) })
                    .text_color(gpui::rgb(0x555555))
                    .child(header),
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
            KeyBinding::new("cmd-z", Undo, None),
            KeyBinding::new("cmd-shift-z", Redo, None),
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
            // Document start/end (macOS ⌘↑/⌘↓) and page motion, ⇧ to select.
            KeyBinding::new("cmd-up", DocStart, None),
            KeyBinding::new("cmd-down", DocEnd, None),
            KeyBinding::new("cmd-shift-up", SelectDocStart, None),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, None),
            KeyBinding::new("pageup", PageUp, None),
            KeyBinding::new("pagedown", PageDown, None),
            KeyBinding::new("shift-pageup", SelectPageUp, None),
            KeyBinding::new("shift-pagedown", SelectPageDown, None),
            KeyBinding::new("escape", Cancel, None),
        ]);
        // Quit is handled on the Editor (to confirm unsaved changes); it falls
        // back to an immediate quit only when no document is open.

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
                        last_row_count: 0,
                        goal_x: None,
                        goal_caret: usize::MAX,
                        quit_armed: false,
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

#[cfg(test)]
mod tests {
    use super::locate_caret_core;

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
}
