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
use std::path::PathBuf;

use dioxus::prelude::*;
use leaf_core::{Doc, View};

use crate::host::Host;
use crate::media::{self, MediaBlock};
use crate::picture::{self, PictureBlock};
use crate::row::{Pointer, TableBlock};
use crate::table;
use crate::text::{Fonts, byte_of_col};

/// Something drawn in place of rows.
#[derive(Clone, PartialEq)]
pub enum Block {
    Table(table::Block),
    Media(media::Block),
    Picture(picture::Block),
}

impl Block {
    /// The rows it stands for.
    pub fn rows(&self) -> Range<usize> {
        match self {
            Block::Table(t) => t.rows.clone(),
            Block::Media(m) => m.rows.clone(),
            Block::Picture(p) => p.rows.clone(),
        }
    }

    /// The row it is drawn at.
    pub fn first(&self) -> usize {
        match self {
            Block::Table(t) => t.first,
            Block::Media(m) => m.first,
            Block::Picture(p) => p.first,
        }
    }

    /// How tall it draws, in CSS pixels.
    pub fn height(&self) -> f64 {
        match self {
            Block::Table(t) => t.layout.height as f64,
            Block::Media(m) => m.height() as f64,
            Block::Picture(p) => p.height() as f64,
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
            (Block::Media(m), crate::row::Target::Block { .. }) => Some(m.hit(x)),
            (Block::Picture(b), crate::row::Target::Block { .. }) => Some(b.hit(x)),
            _ => None,
        }
    }
}

/// Where a medium's source is on disk: the host's reading, else the editor's
/// own (`media::resolve`) against the document's directory.
pub fn locate(host: &dyn Host, doc: &Doc, dest: &str) -> Option<PathBuf> {
    host.locate_media(doc, dest)
        .or_else(|| media::resolve(dest, doc.path.parent()))
}

/// Every block in the document as it stands, reusing last frame's where
/// nothing about one has changed.
#[allow(clippy::too_many_arguments)]
pub fn build(
    doc: &Doc,
    host: &dyn Host,
    caret: usize,
    composing: Option<(usize, usize)>,
    fonts: &mut Fonts,
    width: f32,
    dark: bool,
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
            Some(b) => table::Block {
                first: t.rows_span.start,
                rows: t.rows_span.clone(),
                ..b
            },
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

    // Pictures, video and audio, in place of their placeholder rows. A box
    // is a few numbers, worked out each frame from the sizes `media.rs`
    // caches by path; only its prefix's layout is kept from the last. The
    // host reads each source first; with no opinion, the editor's own rule.
    let locate = |dest: &str| locate(host, doc, dest);
    for m in &doc.vmap.media {
        let reuse = |p: &_| {
            old.iter().find_map(|b| match b {
                Block::Media(b) if b.prefix == *p => Some(b.prefix_layout.clone()),
                _ => None,
            })
        };
        if let Some(b) = media::block(m, &doc.vmap.rows, &locate, caret, sel, fonts, width, reuse) {
            out.push(Block::Media(b));
        }
    }

    // Display formulas and Mermaid diagrams, as their pictures.
    let size = fonts.ty().size;
    for m in &doc.vmap.math {
        if let Some(b) = picture::math_block(m, &doc.vmap.rows, caret, sel, size, dark, width) {
            out.push(Block::Picture(b));
        }
    }
    for c in &doc.vmap.code_blocks {
        if let Some(b) =
            picture::diagram_block(c, &doc.vmap.rows, &doc.source, caret, sel, dark, width)
        {
            out.push(Block::Picture(b));
        }
    }

    *cache.borrow_mut() = out.clone();
    out
}

/// A block's markup, at row `i`.
pub fn render(i: usize, block: &Block) -> Element {
    match block {
        Block::Table(b) => rsx! { TableBlock { key: "{i}", block: b.clone() } },
        Block::Media(b) => rsx! { MediaBlock { key: "{i}", block: b.clone() } },
        Block::Picture(b) => rsx! { PictureBlock { key: "{i}", block: b.clone() } },
    }
}
