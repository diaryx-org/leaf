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
use crate::viewport::LINE;

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
        if !extend && st.doc.borrow_mut().caret_in_table() && st.doc.borrow_mut().cell_move_vertical(down) {
            st.goal.set(None);
            return;
        }
        let Some((row, line, x)) = self.caret_at() else { return };
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
            (0..row).rev().find(|&r| holds(r)).map(|r| (r, lays[r].0.lines.len() - 1))
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
        let lines = ((self.view_height() / LINE) as usize).saturating_sub(1).max(1);
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
    /// there if the document ends in a block Return cannot leave.
    pub fn past_end(&self) {
        self.st.doc.borrow_mut().click_past_end();
        self.refresh();
    }

    /// A pointer event on a line of a row or of a block.
    pub fn pointer(&self, p: Pointer) {
        let st = &*self.st;
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
            Pointer::Press { at, line, x, .. } | Pointer::Drag { at, line, x } => (at, line, x),
        };
        let (row, key, spot) = match at {
            Target::Row(row) => {
                let Some(col) = self.sig.lays.peek().get(row).map(|l| l.0.col_at(line, x)) else {
                    return;
                };
                (row, col, Spot::Pos(row, col))
            }
            Target::Cell { first, .. } => {
                let blocks = self.sig.blocks.peek();
                let Some(off) = blocks.iter().find(|b| b.first() == first).and_then(|b| b.hit(&p)) else {
                    return;
                };
                (first, off, Spot::Offset(off))
            }
        };
        let mode = match p {
            Pointer::Press { mode, .. } => mode,
            Pointer::Drag { .. } => PressMode::Place,
        };
        match mode {
            PressMode::Place => gesture(&mut st.doc.borrow_mut(), &st.last_press, p, row, key, spot),
            // ⌘-click places the caret, then follows what it landed on.
            PressMode::Follow => place(&mut st.doc.borrow_mut(), spot, false),
            // A right-click outside the selection moves the caret there
            // first, like a native text view; one inside keeps it.
            PressMode::Context => {
                let mut doc = st.doc.borrow_mut();
                let at = spot_offset(&doc, spot);
                let inside = doc.anchor.is_some_and(|a| a.min(doc.caret) <= at && at <= a.max(doc.caret));
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
fn gesture(doc: &mut Doc, last: &std::cell::Cell<Option<Press>>, p: Pointer, row: usize, key: usize, spot: Spot) {
    let offset = |doc: &Doc| spot_offset(doc, spot);
    let place = |doc: &mut Doc, extend: bool| place(doc, spot, extend);
    match p {
        Pointer::Press { extend, .. } => {
            let now = Instant::now();
            let count = match last.get() {
                Some(l) if !extend && l.row == row && now - l.at < MULTI_CLICK => l.count % 3 + 1,
                _ => 1,
            };
            last.set(Some(Press { at: now, row, key, count }));
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
        Pointer::Drag { .. } => {
            // A move that has not left the pressed spot is the hand settling,
            // not a drag — and would shrink a double-click's word back to the
            // pointer. A drag after a double- or triple-click should extend by
            // words or blocks, as Swift's does; until it does, it is left
            // alone rather than undoing the selection.
            match last.get() {
                Some(p) if (p.row, p.key) == (row, key) || p.count > 1 => {}
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
