//! Blocks: what the editor draws in place of rows.
//!
//! Core lays every block out as rows, which a monospace surface can paint as
//! they come. Some of them a frontend with its own geometry draws its own way
//! instead — a table as a grid (`table.rs`); an image, a formula or a diagram
//! as a picture — and core says which rows each one spans (`TableInfo`,
//! `MediaInfo`, `MathInfo`, …). Each such thing is a [`Block`]: it stands in
//! for its rows, its first row holding its whole height and the rest none, so
//! row indices still match core's map.
//!
//! Adding a kind of block is this file alone: a variant of [`Block`], a step
//! in [`build`], an arm in [`render`] and in [`Block::hit`] — and its own
//! module for what it draws. The editor's repaint, scrolling and pointer
//! handling go through these four and do not change.

use std::cell::RefCell;
use std::ops::Range;

use dioxus::prelude::*;
use leaf_core::{Doc, View};

use crate::row::{Pointer, TableBlock};
use crate::table;
use crate::text::{Fonts, byte_of_col};

/// Something drawn in place of rows.
#[derive(Clone, PartialEq)]
pub enum Block {
    Table(table::Block),
}

impl Block {
    /// The rows it stands for.
    pub fn rows(&self) -> Range<usize> {
        match self {
            Block::Table(t) => t.rows.clone(),
        }
    }

    /// The row it is drawn at.
    pub fn first(&self) -> usize {
        match self {
            Block::Table(t) => t.first,
        }
    }

    /// How tall it draws, in CSS pixels.
    pub fn height(&self) -> f64 {
        match self {
            Block::Table(t) => t.layout.height as f64,
        }
    }

    /// The source offset a pointer event on it lands on, if any — the
    /// glyph under it, never a column of core's picture.
    pub fn hit(&self, p: &Pointer) -> Option<usize> {
        let (at, line, x) = match *p {
            Pointer::Press { at, line, x, .. } | Pointer::Drag { at, line, x } => (at, line, x),
        };
        match (self, at) {
            (Block::Table(t), crate::row::Target::Cell { r, c, .. }) => {
                let cell = t.view.rows.get(r)?.cells.get(c)?;
                let cl = t.layout.rows.get(r)?.cells.get(c)?.as_ref()?;
                let text: String = cell.view.runs.iter().map(|r| r.text.as_str()).collect();
                Some(cell.offset(byte_of_col(&text, cl.col_at(line, x))))
            }
            _ => None,
        }
    }
}

/// Every block in the document as it stands, reusing last frame's where
/// nothing about one has changed.
pub fn build(
    doc: &Doc,
    caret: usize,
    composing: Option<(usize, usize)>,
    fonts: &mut Fonts,
    width: f32,
    cache: &RefCell<Vec<Block>>,
) -> Vec<Block> {
    if doc.view == View::Source {
        cache.borrow_mut().clear();
        return Vec::new();
    }
    let old = std::mem::take(&mut *cache.borrow_mut());
    let sel = doc.selection();
    let mut out = Vec::new();

    // Tables, as grids. A table whose view and width are as they were keeps
    // its layout.
    for t in &doc.vmap.tables {
        let view = table::view_of(t, caret, sel, composing);
        let reused = old.iter().find_map(|b| match b {
            Block::Table(b) if *b.view == view && b.layout_width == width => Some(b.clone()),
            _ => None,
        });
        let block = match reused {
            Some(b) => table::Block { first: t.rows_span.start, rows: t.rows_span.clone(), ..b },
            None => {
                let layout = table::layout(fonts, &view, width);
                table::Block {
                    first: t.rows_span.start,
                    rows: t.rows_span.clone(),
                    view: std::rc::Rc::new(view),
                    layout: std::rc::Rc::new(layout),
                    layout_width: width,
                }
            }
        };
        out.push(Block::Table(block));
    }

    *cache.borrow_mut() = out.clone();
    out
}

/// A block's markup, at row `i`.
pub fn render(i: usize, block: &Block) -> Element {
    match block {
        Block::Table(b) => rsx! { TableBlock { key: "{i}", block: b.clone() } },
    }
}
