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
use std::rc::Rc;

use dioxus::prelude::*;
use leaf_core::{Doc, View};

use crate::bridge::rows::RowHandle;
use crate::host::Host;
use crate::media::{self, MediaBlock};
use crate::picture::{self, PictureBlock};
use crate::row::{LayoutHandle, Pointer, Row, TableBlock};
use crate::table;
use crate::text::{Fonts, Kind, byte_of_col};

/// The gap between a directive's label and the host's card under it.
const CARD_GAP: f32 = 8.0;

/// A directive in the document — `::name[label]{attrs}` — as a host reads
/// it to draw a card for it ([`Host::directive_height`]). Core knows no
/// vocabulary of directives; what each one means is the host's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    /// Its type, with no leading colons.
    pub name: String,
    /// Its `{…}` attributes in source order; a bare one has a `None` value.
    pub attrs: Vec<(String, Option<String>)>,
    /// Its `[label]` text, flattened.
    pub label: String,
}

impl Directive {
    /// The value of the attribute `key`, if it has one.
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_deref())
    }
}

/// A directive the host draws a card for: its label row, drawn and hit as
/// any row is, and the card under it.
#[derive(Clone, PartialEq)]
pub struct DirectiveBlock {
    pub first: usize,
    pub rows: Range<usize>,
    row: RowHandle,
    layout: LayoutHandle,
    kind: Kind,
    pub directive: Directive,
    /// The card's height, as the host answered it.
    card: f32,
}

/// Something drawn in place of rows.
#[derive(Clone, PartialEq)]
pub enum Block {
    Table(table::Block),
    Media(media::Block),
    Picture(picture::Block),
    Directive(DirectiveBlock),
}

impl Block {
    /// The rows it stands for.
    pub fn rows(&self) -> Range<usize> {
        match self {
            Block::Table(t) => t.rows.clone(),
            Block::Media(m) => m.rows.clone(),
            Block::Picture(p) => p.rows.clone(),
            Block::Directive(d) => d.rows.clone(),
        }
    }

    /// The row it is drawn at.
    pub fn first(&self) -> usize {
        match self {
            Block::Table(t) => t.first,
            Block::Media(m) => m.first,
            Block::Picture(p) => p.first,
            Block::Directive(d) => d.first,
        }
    }

    /// How tall it draws, in CSS pixels.
    pub fn height(&self) -> f64 {
        match self {
            Block::Table(t) => t.layout.height as f64,
            Block::Media(m) => m.height() as f64,
            Block::Picture(p) => p.height() as f64,
            Block::Directive(d) => (d.layout.0.height + CARD_GAP + d.card) as f64,
        }
    }

    /// The source offset a pointer event on it lands on, if any — the
    /// glyph under it, never a column of core's picture.
    pub fn hit(&self, p: &Pointer) -> Option<usize> {
        let (at, line, x) = match *p {
            Pointer::Press { at, line, x, .. }
            | Pointer::Drag { at, line, x }
            | Pointer::Tap { at, line, x } => (at, line, x),
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

/// What a medium draws: the picture the host loaded itself, else the file
/// [`locate`] finds, measured once.
pub fn find_still(host: &dyn Host, doc: &Doc, dest: &str) -> Option<Rc<media::Still>> {
    if let Some(p) = host.picture(doc, dest) {
        return Some(Rc::new(media::Still {
            url: p.url,
            size: p.size,
        }));
    }
    locate(host, doc, dest).and_then(|p| media::still(&p))
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
    let find = |dest: &str| find_still(host, doc, dest);
    for m in &doc.vmap.media {
        let reuse = |p: &_| {
            old.iter().find_map(|b| match b {
                Block::Media(b) if b.prefix == *p => Some(b.prefix_layout.clone()),
                _ => None,
            })
        };
        if let Some(b) = media::block(m, &doc.vmap.rows, &find, caret, sel, fonts, width, reuse) {
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

/// Every directive the host draws a card for, given the frame's rows and
/// their layouts — its label row is one of them, drawn as it is.
pub fn directives(
    doc: &Doc,
    host: &dyn Host,
    handles: &[RowHandle],
    lays: &[LayoutHandle],
    kinds: &[Kind],
) -> Vec<Block> {
    if doc.view == View::Source {
        return Vec::new();
    }
    doc.vmap
        .directives
        .iter()
        .filter_map(|d| {
            let first = d.rows_span.start;
            let directive = Directive {
                name: d.name.clone(),
                attrs: d.attrs.clone(),
                label: d.label.clone(),
            };
            let card = host.directive_height(doc, &directive)?;
            Some(Block::Directive(DirectiveBlock {
                first,
                rows: d.rows_span.clone(),
                row: handles.get(first)?.clone(),
                layout: lays.get(first)?.clone(),
                kind: *kinds.get(first)?,
                directive,
                card,
            }))
        })
        .collect()
}

/// A block's markup, at row `i`; `host` draws a directive's card.
pub fn render(i: usize, block: &Block, host: &dyn Host) -> Element {
    match block {
        Block::Directive(d) => rsx! {
            div { key: "b{i}", class: "directive",
                Row { index: i, row: d.row.clone(), layout: d.layout.clone(), kind: d.kind }
                div { class: "card", style: "height: {d.card}px; margin-top: {CARD_GAP}px",
                    {host.draw_directive(&d.directive)}
                }
            }
        },
        Block::Table(b) => rsx! { TableBlock { key: "b{i}", block: b.clone() } },
        Block::Media(b) => rsx! { MediaBlock { key: "b{i}", block: b.clone() } },
        Block::Picture(b) => rsx! { PictureBlock { key: "b{i}", block: b.clone() } },
    }
}
