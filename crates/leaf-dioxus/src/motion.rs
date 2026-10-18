//! Where the caret is in the layout, the motion only the layout can do, and
//! the pointer.
//!
//! Core moves the caret through its own map; but rows here are wrapped by the
//! editor (`text.rs`) and tables drawn by it (`table.rs`), so moving by a
//! *visual* line, to a visual line's ends, and from a click's x to a position
//! all go through the editor's layouts — Swift's split, with CTTypesetter in
//! parley's place.

use std::time::{Duration, Instant};

use dioxus::prelude::*;
use leaf_core::{Doc, View};

use crate::editor::Editor;
use crate::find::Focus;
use crate::row::{Pointer, PressMode, Target};
use crate::source;
use crate::trace::trace;

/// Presses closer together than this, on the same row, count as one double-
/// or triple-click — the system default on macOS.
const MULTI_CLICK: Duration = Duration::from_millis(500);

/// The last press, for counting a double- and triple-click.
#[derive(Clone, Copy)]
pub struct Press {
    at: Instant,
    row: usize,
    key: usize,
    count: u8,
    /// Whether a drag after it selects. Not after a ⌘-click or a
    /// right-click: the hand settling would extend from wherever following
    /// the link put the caret.
    drags: bool,
}

/// Where a pointer gesture landed, resolved: a (row, column) of the view, or
/// — on a block, drawn by the editor rather than from the map's rows — the
/// source offset of the glyph under it.
#[derive(Clone, Copy, Debug)]
pub enum Spot {
    Pos(usize, usize),
    Offset(usize),
}

/// A source offset's (row, column) in whichever view is showing.
pub fn pos_of(doc: &Doc, offset: usize) -> (usize, usize) {
    match doc.view {
        View::Source => source::pos_of(doc, offset),
        View::Wysiwyg => doc.vmap.pos_of_offset(offset),
    }
}

/// The source offset at (row, column) of whichever view is showing.
pub fn offset_at(doc: &Doc, row: usize, col: usize) -> usize {
    match doc.view {
        View::Source => source::offset_at(doc, row, col),
        View::Wysiwyg => doc.vmap.offset_of_pos(row, col),
    }
}

impl Editor {
    /// The source offset of the character at (`x`, `y`), in CSS pixels from
    /// the window's top-left — the inverse of [`ime_area`](Self::ime_area)'s
    /// arithmetic, and the column a click there would place the caret at.
    /// `None` off the rows, and on a block (a table, a picture), whose
    /// geometry is its own.
    pub fn offset_at_point(&self, x: f64, y: f64) -> Option<usize> {
        let layout = self.sig.layout.peek();
        let doc_y =
            y - self.sig.origin.peek().1 - self.sig.bar.peek().height() + *self.sig.top.peek();
        let row =
            (0..layout.rows()).find(|&r| doc_y >= layout.top(r) && doc_y < layout.bottom(r))?;
        if self.in_block(row) {
            return None;
        }
        let lays = self.sig.lays.peek();
        let lay = &lays.get(row)?.0;
        let within = (doc_y - layout.top(row)) as f32;
        let line = lay
            .lines
            .iter()
            .position(|l| within < l.top + l.height)
            .unwrap_or(lay.lines.len().saturating_sub(1));
        let col = lay.col_at(line, (x - self.left()) as f32);
        Some(offset_at(&self.st.doc.borrow(), row, col))
    }

    /// Where the source range `from`..`to` is drawn, as (x, y, width,
    /// height) in CSS pixels from the window's top-left: its first visual
    /// line's part of it, the line's full height. `None` on a block, or
    /// where no row is laid out.
    pub fn rect_of(&self, from: usize, to: usize) -> Option<(f64, f64, f64, f64)> {
        let ((row, c0), (row_end, c1)) = {
            let doc = self.st.doc.borrow();
            (pos_of(&doc, from), pos_of(&doc, to))
        };
        if self.in_block(row) {
            return None;
        }
        let lays = self.sig.lays.peek();
        let lay = &lays.get(row)?.0;
        let (line, x0) = lay.caret(c0);
        let x1 = match (row_end == row).then(|| lay.caret(c1)) {
            Some((l, x)) if l == line => x,
            // Past the line: to the column's edge.
            _ => *self.sig.width.peek(),
        };
        let l = lay.lines.get(line)?;
        let y = self.sig.layout.peek().top(row) - *self.sig.top.peek()
            + l.top as f64
            + self.sig.bar.peek().height()
            + self.sig.origin.peek().1;
        Some((
            self.left() + x0 as f64,
            y,
            (x1 - x0).max(0.0) as f64,
            l.height as f64,
        ))
    }

    /// [`rect_of`](Self::rect_of), its line measured where Blitz drew it
    /// rather than where the layout puts it: the two part where the page's
    /// stylesheet has not reached the rows (they are drawn in the bridge's
    /// monospace, and parley measured the look's face), or where something
    /// above the rows takes room the layout does not count. What a host
    /// points at a range with — a system edit menu over a selection — so
    /// that it points at the glyphs on screen. `None` where `rect_of` is, or
    /// where the line is not mounted.
    pub async fn drawn_rect_of(&self, from: usize, to: usize) -> Option<(f64, f64, f64, f64)> {
        let (x, _, w, _) = self.rect_of(from, to)?;
        let (row, col) = pos_of(&self.st.doc.borrow(), from);
        let line = self.sig.lays.peek().get(row)?.0.caret(col).0;
        let data = self
            .st
            .lines
            .0
            .borrow()
            .get(&(Target::Row(row), line))?
            .1
            .clone();
        // Read through the document's shared borrow, not `get_client_rect`,
        // which takes it mutably and panics while it is borrowed — as it is
        // when a window event reaches the host that asks. Busy, it is asked
        // again a frame later, a few times.
        let node = data.downcast::<dioxus_native::NodeHandle>()?;
        for _ in 0..10 {
            if let Some(doc) = node.try_doc() {
                let r = doc.get_client_bounding_rect(node.node_id())?;
                return Some((x, r.y, w, r.height));
            }
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
        None
    }

    /// The face and size the character at source offset `off` is set in,
    /// in CSS pixels. `None` on a block, or where no row is laid out.
    pub fn font_at(&self, off: usize) -> Option<(parley::FontData, f32)> {
        let (row, col) = pos_of(&self.st.doc.borrow(), off);
        if self.in_block(row) {
            return None;
        }
        self.sig.lays.peek().get(row)?.0.font_at(col)
    }

    fn in_block(&self, row: usize) -> bool {
        self.sig
            .blocks
            .peek()
            .iter()
            .any(|b| !matches!(b, crate::blocks::Block::Directive(_)) && b.rows().contains(&row))
    }

    /// Where the caret is in the layout: its row, its visual line, its x.
    pub fn caret_at(&self) -> Option<(usize, usize, f32)> {
        let doc = self.st.doc.borrow();
        let at = self.st.composer.borrow().caret().unwrap_or(doc.caret);
        let (row, col) = pos_of(&doc, at);
        let lays = self.sig.lays.peek();
        let (line, x) = lays.get(row)?.0.caret(col);
        Some((row, line, x))
    }

    /// One visual line up or down, by geometry: the line above or below in
    /// this row, or the nearest caret-holding row's last or first line, at
    /// the goal x. Swift's rule, which core cannot apply to rows it never
    /// wrapped.
    pub fn vertical(&self, down: bool, extend: bool) {
        let st = &*self.st;
        // In a table, up and down are core's: to the cell above or below, in
        // the same column — falling through at the grid's edge.
        if !extend
            && st.doc.borrow_mut().caret_in_table()
            && st.doc.borrow_mut().cell_move_vertical(down)
        {
            st.goal.set(None);
            return;
        }
        let Some((row, line, x)) = self.caret_at() else {
            return;
        };
        let aim = st.goal.get().unwrap_or(x);
        let lays = self.sig.lays.peek();
        let rows = self.sig.rows.peek();
        let holds = |r: usize| rows.get(r).is_some_and(|h| !h.0.decoration);
        let target = if down {
            if line + 1 < lays[row].0.lines.len() {
                Some((row, line + 1))
            } else {
                (row + 1..rows.len()).find(|&r| holds(r)).map(|r| (r, 0))
            }
        } else if line > 0 {
            Some((row, line - 1))
        } else {
            (0..row)
                .rev()
                .find(|&r| holds(r))
                .map(|r| (r, lays[r].0.lines.len() - 1))
        };
        let mut doc = st.doc.borrow_mut();
        match target {
            Some((r, l)) => doc.click(r, lays[r].0.col_at(l, aim), extend),
            // Past the first or last line: to the document's edge, as a text
            // view does.
            None if down => doc.move_doc_end(extend),
            None => doc.move_doc_start(extend),
        }
        st.goal.set(Some(aim));
    }

    /// A page up or down: the viewport less a line of overlap, in body lines
    /// — Swift's and gpui's rule.
    pub fn page(&self, down: bool, extend: bool) {
        let line = self.st.fonts.borrow().ty().line() as f64;
        let lines = ((self.view_height() / line) as usize)
            .saturating_sub(1)
            .max(1);
        for _ in 0..lines {
            self.vertical(down, extend);
        }
    }

    /// To the start or end of the visual line the caret is on.
    pub fn line_edge(&self, end: bool, extend: bool) {
        if let Some((row, line, _)) = self.caret_at() {
            let col = self.sig.lays.peek()[row].0.edge(line, end);
            self.st.doc.borrow_mut().click(row, col, extend);
        }
    }

    /// A click in the blank space under the last row: core opens a paragraph
    /// there if the document ends in a block Return cannot leave. `tap` says
    /// whether it was a finger's tap or a mouse's press, of which the editor
    /// hears only the one it is listening for (see [`pointer`](Self::pointer)).
    pub fn past_end(&self, tap: bool) {
        if tap != self.st.touch_mode.get() || (tap && self.st.gripped.get()) {
            return;
        }
        if tap {
            self.enable_ime();
        }
        self.st.doc.borrow_mut().click_past_end();
        self.refresh();
    }

    /// A pointer event on a line of a row or of a block. A mouse is heard by
    /// its press and drag, a finger by its tap (`touch.rs`), and each ignores
    /// the other's; a tap also asks for the keyboard.
    pub fn pointer(&self, p: Pointer) {
        let st = &*self.st;
        let p = match p {
            // A finger let go of a grab handle, not tapped the text.
            Pointer::Tap { .. } if st.gripped.get() => return,
            Pointer::Tap { at, line, x } if st.touch_mode.get() => {
                self.enable_ime();
                Pointer::Press {
                    at,
                    line,
                    x,
                    extend: false,
                    mode: PressMode::Place,
                }
            }
            Pointer::Tap { .. } => return,
            _ if st.touch_mode.get() => return,
            p => p,
        };
        let p = p.shifted((crate::row::PAD - self.left()) as f32);
        st.goal.set(None);
        st.composer.borrow_mut().settle(&mut st.doc.borrow_mut());
        // A click in the document takes typing back from the find bar, and
        // leaves the bar open, as on a Mac.
        let refocus = {
            let mut f = st.find.borrow_mut();
            let was = f.focus != Focus::Doc;
            f.focus = Focus::Doc;
            was
        };
        if refocus {
            self.sync_bar();
        }
        let (at, line, x) = match p {
            Pointer::Press { at, line, x, .. }
            | Pointer::Drag { at, line, x }
            | Pointer::Tap { at, line, x } => (at, line, x),
        };
        let (row, key, spot) = match at {
            Target::Row(row) => {
                let Some(col) = self.sig.lays.peek().get(row).map(|l| l.0.col_at(line, x)) else {
                    return;
                };
                (row, col, Spot::Pos(row, col))
            }
            Target::Cell { first, .. } | Target::Block { first } => {
                let blocks = self.sig.blocks.peek();
                let Some(off) = blocks
                    .iter()
                    .find(|b| b.first() == first)
                    .and_then(|b| b.hit(&p))
                else {
                    return;
                };
                (first, off, Spot::Offset(off))
            }
        };
        let mode = match p {
            Pointer::Press { mode, .. } => mode,
            Pointer::Drag { .. } | Pointer::Tap { .. } => PressMode::Place,
        };
        if mode != PressMode::Place {
            st.last_press.set(Some(Press {
                at: Instant::now(),
                row,
                key,
                count: 1,
                drags: false,
            }));
        }
        match mode {
            PressMode::Place => {
                gesture(&mut st.doc.borrow_mut(), &st.last_press, p, row, key, spot)
            }
            // ⌘-click places the caret, then follows what it landed on.
            PressMode::Follow => place(&mut st.doc.borrow_mut(), spot, false),
            // A right-click outside the selection moves the caret there
            // first, like a native text view; one inside keeps it.
            PressMode::Context => {
                let mut doc = st.doc.borrow_mut();
                let at = spot_offset(&doc, spot);
                let inside = doc
                    .anchor
                    .is_some_and(|a| a.min(doc.caret) <= at && at <= a.max(doc.caret));
                if !inside {
                    place(&mut doc, spot, false);
                }
            }
        }
        trace(|| {
            let d = st.doc.borrow();
            format!("{p:?} -> {spot:?}, caret {} anchor {:?}", d.caret, d.anchor)
        });
        match mode {
            PressMode::Place => {
                self.repaint();
            }
            PressMode::Follow => {
                self.repaint();
                self.follow_at_caret();
            }
            PressMode::Context => {
                self.repaint();
                self.context_menu();
            }
        }
    }
}

/// Apply a pointer gesture at a resolved spot: a press places the caret, or
/// on a task box ticks it; a second and third press in quick succession
/// select the word and the block; a drag extends the selection from wherever
/// the press left it. `row` and `key` identify the spot for counting presses
/// and noticing that a drag has not moved.
fn gesture(
    doc: &mut Doc,
    last: &std::cell::Cell<Option<Press>>,
    p: Pointer,
    row: usize,
    key: usize,
    spot: Spot,
) {
    let offset = |doc: &Doc| spot_offset(doc, spot);
    let place = |doc: &mut Doc, extend: bool| place(doc, spot, extend);
    match p {
        Pointer::Press { extend, .. } => {
            let now = Instant::now();
            let count = match last.get() {
                Some(l) if !extend && l.row == row && now - l.at < MULTI_CLICK => l.count % 3 + 1,
                _ => 1,
            };
            last.set(Some(Press {
                at: now,
                row,
                key,
                count,
                drags: true,
            }));
            if count == 1
                && !extend
                && doc.view == View::Wysiwyg
                && let Spot::Pos(r, c) = spot
                && let Some(offset) = doc.vmap.task_box_at(r, c)
            {
                // A tick, not a caret placement: the caret stays where it was,
                // as it does in the Swift editor.
                doc.toggle_task_at(offset);
                return;
            }
            match count {
                2 => {
                    let at = offset(doc);
                    doc.select_word_at(at);
                }
                3 => {
                    let at = offset(doc);
                    doc.select_block_at(at);
                }
                _ => place(doc, extend),
            }
        }
        // A tap is a press by the time it gets here.
        Pointer::Tap { .. } => {}
        Pointer::Drag { .. } => {
            // A move that has not left the pressed spot is the hand settling,
            // not a drag — and would shrink a double-click's word back to the
            // pointer. A drag after a double- or triple-click should extend by
            // words or blocks, as Swift's does; until it does, it is left
            // alone rather than undoing the selection.
            match last.get() {
                Some(p) if (p.row, p.key) == (row, key) || p.count > 1 || !p.drags => {}
                _ => place(doc, true),
            }
        }
    }
}

/// The source offset a resolved spot stands for.
fn spot_offset(doc: &Doc, spot: Spot) -> usize {
    match spot {
        Spot::Pos(r, c) => offset_at(doc, r, c),
        Spot::Offset(o) => o,
    }
}

/// Put the caret at a resolved spot, or extend the selection to it.
fn place(doc: &mut Doc, spot: Spot, extend: bool) {
    match spot {
        Spot::Pos(r, c) => doc.click(r, c, extend),
        Spot::Offset(o) => doc.place_caret(o, extend),
    }
}
