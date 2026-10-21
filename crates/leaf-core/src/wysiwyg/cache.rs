//! The per-block layout cache behind incremental builds, and shifting a cached
//! row to a new offset.

use super::*;

/// A persistent, content-keyed cache of the rows each top-level block renders
/// to — the [`VisualMap`] analogue of the GUI's ShapedLine cache, one level
/// down. Held by a [`crate::Doc`] and threaded into [`build_cached`], it is what
/// makes a rebuild after a keystroke cost "re-render the edited block + shift
/// the rest" instead of re-rendering the whole document.
///
/// A top-level block's rows are a pure function of its kind, its source bytes
/// and the wrap width, so an unchanged block's rows are cloned and their source offsets
/// shifted by the edit's byte delta rather than rebuilt glyph by glyph. Two
/// things make that purity hold: at the top level the render prefix is always
/// empty (nesting prefixes — a quote gutter, a list indent — exist only *inside*
/// a top-level block, within its cached unit), and a block's output never reads
/// the incoming `last_off` (it writes `last_off` from its own content before any
/// nested separator reads it). So the only thing that differs between two
/// positions of an unchanged block is a uniform offset shift. Keyed by a fast
/// hash of the block's bytes with the bytes kept for a verify-on-hit — exactly
/// the shape cache's weak-hash-then-compare, so a collision costs a re-render,
/// never a wrong row.
///
/// Tables are never cached (a block that emits any table row is always rebuilt):
/// their rows are cross-referenced from the map's `tables` side-table by row
/// index, which a blind offset-shift wouldn't fix up, and they are rare enough
/// that the simplicity beats the reuse.
#[derive(Default)]
pub struct BlockCache {
    /// The wrap width every entry was built at; a change invalidates all of
    /// them. `None` before the first build (distinct from `Some(None)`, the
    /// unwrapped GUI width).
    pub(super) wrap: Option<Option<usize>>,
    /// Bumped once per [`build_cached`]. An entry reused or inserted this build
    /// carries the current value; stale entries are dropped at the end of it.
    pub(super) generation: u64,
    /// `hash(bytes)` → the block(s) sharing that hash — a bucket because
    /// distinct blocks can collide, while two *identical* blocks share one entry
    /// (free dedup).
    pub(super) entries: HashMap<u64, Vec<CachedBlock>>,
    /// The row/stop decomposition of the last build, which [`build_spliced`]
    /// patches in place for a single-block edit. Kept in step with whatever
    /// [`VisualMap`] was last produced; empty before the first build.
    pub(super) layout: Layout,
}

/// How the last build's [`VisualMap`] decomposes into top-level blocks — the
/// bookkeeping [`build_spliced`] needs to splice one block's rows and stops
/// without rebuilding the whole map. Every field describes the *previous* build,
/// in that build's coordinates.
#[derive(Default)]
pub(super) struct Layout {
    /// One entry per rendered (metadata-filtered) top-level block, in order.
    pub(super) blocks: Vec<BlockLayout>,
    /// Trailing blank rows past the last block (from `emit_trailing_blank_lines`).
    pub(super) trailing_rows: usize,
    /// The source length this layout was built at — the reference for the edit's
    /// byte delta.
    pub(super) built_len: usize,
    /// Whether the last build drew any table. A table's cross-referenced row
    /// indices don't survive a blind splice, so their presence makes
    /// [`build_spliced`] bail to a full rebuild.
    pub(super) has_tables: bool,
    /// Whether every block rendered strictly inside its own span (see
    /// [`rows_within`]). A block that doesn't — a malformed Markdown inline node
    /// that twig leaves with a degenerate `0..0` span renders at a fixed offset
    /// outside its block — can't be shifted correctly, so its presence makes
    /// [`build_spliced`] bail to a full rebuild.
    pub(super) all_shift_safe: bool,
    /// The reveal line this layout was built under (see [`Builder::reveal`]).
    /// A splice reuses every row it isn't re-rendering, so a reveal line that
    /// has moved would leave the old line still showing its delimiters and the
    /// new one still hiding them — [`build_spliced`] bails when this changes.
    pub(super) reveal: Option<Reveal>,
}

/// One top-level block's contribution to the last build: its span and kind (for
/// the structural match that proves only one block changed) and how many
/// separator and content rows it emitted (to locate its slice of the row
/// vector).
pub(super) struct BlockLayout {
    pub(super) span: Range<usize>,
    pub(super) kind: Kind,
    pub(super) sep_rows: usize,
    pub(super) content_rows: usize,
    /// Whether the block holds a formula — see [`BlockCache::math_meets`].
    pub(super) has_math: bool,
}

/// One cached block: the rows it rendered to, plus what a reuse at a new
/// position needs to shift them. Offsets are stored absolute (as built) and
/// shifted by `new_start - built_start` on reuse.
pub(super) struct CachedBlock {
    /// The block's exact source bytes, compared on a hash hit so a collision
    /// can never hand back another block's rows.
    bytes: Box<[u8]>,
    /// The block's kind, compared with `bytes`: the same bytes parse as
    /// different blocks in different places. A paragraph `after` drawn one
    /// keystroke, and the same five bytes glued under a `</div>` the next —
    /// a bare `str` at the top level, which draws nothing — share a hash and
    /// bytes, and only the kind tells their rows apart.
    kind: Kind,
    /// The offset the rows were built at (the block's `span.start`).
    pub(super) built_start: usize,
    /// The block's rows, offsets absolute as built.
    pub(super) rows: Vec<VRow>,
    /// `last_off` after this block was emitted, absolute as built — restored
    /// (shifted) on reuse so the following separator lands correctly.
    pub(super) last_off: usize,
    /// `stepped_over` after this block was emitted, absolute as built — the
    /// hidden tail a block ends with (a `</div>`), restored with `last_off`
    /// so the trailing blank lines are counted from past it on a hit too.
    pub(super) stepped_over: usize,
    /// Where the reveal line fell *within this block* when the rows were built,
    /// as a block-relative byte range — see [`reveal_key`]. Compared alongside
    /// `bytes` on a hit, because identical source renders to different rows
    /// depending on whether the caret's line is inside it: the same `*em*`
    /// shows its asterisks on the revealed line and hides them everywhere else.
    ///
    /// Block-relative rather than absolute so an unaffected block still hits
    /// after an edit shifts it, and `None` for the overwhelmingly common
    /// no-reveal case — which is why an entry stored under `MarkupMode::None`
    /// keeps hitting for every block that isn't the caret's.
    reveal: Option<Range<usize>>,
    /// Whether the block holds a formula, so a hit can say so to the layout
    /// without walking the rows it is re-emitting.
    pub(super) has_math: bool,
    /// The named families this block's glyphs are set in — see
    /// [`VisualMap::faces`]. Stored with the rows because a hit re-emits them
    /// without walking a `data-font` again, and the map still has to be able to
    /// say what the id on a reused glyph names. Empty for every block that
    /// names no family, which is nearly all of them.
    pub(super) faces: FaceTable,
    /// The build that last reused or inserted this entry (see `generation`).
    pub(super) generation: u64,
}

/// Where `reveal` falls inside a block, in block-relative bytes — the extra key
/// a cached block is stored and matched under.
///
/// `None` when the block doesn't meet the reveal line at all, which is every
/// block on every build in the two hidden modes, and all but one of them under
/// [`crate::MarkupMode::Full`]. So the cache keeps its hit rate as the caret
/// moves: only the line the caret leaves and the line it arrives at re-render.
pub(super) fn reveal_key(reveal: &Option<Reveal>, span: &Range<usize>) -> Option<Range<usize>> {
    let r = reveal.as_ref()?;
    // The same generous intersection test `Builder::revealed` uses, so a block
    // is keyed as revealed exactly when its glyphs will be built that way.
    // Whether the line reveals markup or only math is not in the key: that is
    // a mode, and a mode change empties the cache.
    r.meets(span).then(|| {
        let start = r.line.start.max(span.start) - span.start;
        let end = r.line.end.min(span.end) - span.start;
        start..end
    })
}

impl BlockCache {
    /// Whether the caret's `line` meets a top-level block that holds a
    /// formula, as of the last build — the question [`crate::Doc::reveal_line`]
    /// asks in the hidden markup modes before it threads the line through, so
    /// that only a line with something to reveal costs a rebuild.
    ///
    /// Answered from the layout rather than from twig because the layout is
    /// free: every build records per block whether its walk met a formula
    /// ([`BlockLayout::has_math`]), and a query against the tree is
    /// O(document) on every frame. Coarse on purpose — a block, not a line —
    /// since a block with a formula in it is rare, small, and the one thing
    /// worth re-rendering as the caret moves through it.
    ///
    /// Exact between edits, when the spans are the document's. Across an edit
    /// the spans are the previous revision's, so the caller checks again once
    /// the new layout is in and rebuilds if the answer changed.
    pub(crate) fn math_meets(&self, line: &Range<usize>) -> bool {
        self.layout
            .blocks
            .iter()
            .any(|b| b.has_math && b.span.start <= line.end && line.start <= b.span.end)
    }

    /// Look up a block by hash, verify its kind, bytes and reveal key, and on a
    /// hit stamp it used this build and hand back a borrow to shift-and-clone
    /// from. `None` on a miss (unknown hash, a collision whose bytes differ,
    /// the same bytes parsed as another kind of block, or built under a
    /// different reveal).
    pub(super) fn reuse(
        &mut self,
        hash: u64,
        kind: &Kind,
        bytes: &[u8],
        reveal: &Option<Range<usize>>,
    ) -> Option<&CachedBlock> {
        let g = self.generation;
        let bucket = self.entries.get_mut(&hash)?;
        let e = bucket
            .iter_mut()
            .find(|e| &e.kind == kind && &*e.bytes == bytes && &e.reveal == reveal)?;
        e.generation = g;
        Some(&*e)
    }

    /// Cache the rows a freshly-rendered block produced (or refresh an existing
    /// entry for the same kind, bytes and reveal — an identical block
    /// elsewhere, or a re-render).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn store(
        &mut self,
        hash: u64,
        kind: &Kind,
        bytes: &[u8],
        built_start: usize,
        rows: Vec<VRow>,
        last_off: usize,
        stepped_over: usize,
        reveal: Option<Range<usize>>,
        has_math: bool,
        faces: FaceTable,
    ) {
        let g = self.generation;
        let bucket = self.entries.entry(hash).or_default();
        if let Some(e) = bucket
            .iter_mut()
            .find(|e| &e.kind == kind && &*e.bytes == bytes && e.reveal == reveal)
        {
            e.built_start = built_start;
            e.rows = rows;
            e.last_off = last_off;
            e.stepped_over = stepped_over;
            e.has_math = has_math;
            e.faces = faces;
            e.generation = g;
        } else {
            bucket.push(CachedBlock {
                bytes: bytes.into(),
                kind: kind.clone(),
                built_start,
                rows,
                last_off,
                stepped_over,
                reveal,
                has_math,
                faces,
                generation: g,
            });
        }
    }
}

/// The source bytes a top-level block covers — the block cache's key material.
///
/// Clamped to the source rather than sliced by the span as twig gives it,
/// because that span can end *past* the last byte: the final block of a document
/// with no trailing newline is closed on the virtual newline the parser supplies
/// at EOF, so its `span.end` is `source.len() + 1`. Slicing by such a range
/// yields `None`, and the obvious `unwrap_or(&[])` reads that as *this block has
/// no bytes* — the wrong answer twice over.
///
/// Two blocks whose spans both overrun then key alike, and the second is served
/// the first one's rows. That is not hypothetical: a footnote definition is a
/// root beside `doc` merged back into the top level by [`top_blocks`], while the
/// `section` above it spans the definition's bytes too, so both end at EOF —
/// and a document ending in `[^note]: …` renders that definition as a second
/// copy of the heading. Even alone, a block that keeps hashing empty as the user
/// types in it is served the stale rows built before the edit.
///
/// Clamping hands back the bytes the block really covers, which tells both cases
/// apart, and costs nothing for a span that was in range to begin with.
pub(super) fn block_bytes<'a>(source: &'a str, span: &Range<usize>) -> &'a [u8] {
    let bytes = source.as_bytes();
    let start = span.start.min(bytes.len());
    &bytes[start..span.end.clamp(start, bytes.len())]
}

/// A fast, allocation-free content hash (FNV-1a) for a block's bytes. Weak by
/// design — the bytes are compared on a hit — so its only job is to spread
/// blocks across buckets cheaply. SipHash over every block's bytes on every
/// keystroke would cost more than it saves, the same lesson the shape cache
/// learned when it stopped hashing through the standard hasher.
pub(super) fn block_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in bytes {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Clone a cached row with every source offset advanced by `delta` — the whole
/// cost of reusing an unchanged block: integer adds where a rebuild would
/// re-shape every glyph.
pub(super) fn shift_row(row: &VRow, delta: isize) -> VRow {
    let shift = |off: usize| (off as isize + delta) as usize;
    VRow {
        glyphs: row
            .glyphs
            .iter()
            .map(|g| Glyph {
                ch: g.ch,
                style: g.style,
                src: shift(g.src),
                stop: g.stop,
            })
            .collect(),
        end_src: shift(row.end_src),
        decoration: row.decoration,
        code: row.code,
        code_lang: row.code_lang.clone(),
        directive: row.directive,
        directive_label: row.directive_label.clone(),
        media: row.media.clone(),
        // A tick, not an offset — reuse carries it as-is, like `code_lang`.
        task: row.task,
        leaf_directive: row.leaf_directive.clone(),
        heading: row.heading,
        // Presentation, not offsets: names the author wrote, which a shifted
        // block still wears — like `code_lang`.
        align: row.align,
        line_height: row.line_height,
        hang: row.hang,
        // Structure, not offsets: a reused block's rows divide the same blocks
        // wherever the edit above moved them to.
        boundary: row.boundary,
        mark_ends: row.mark_ends.iter().map(|&o| shift(o)).collect(),
        // Strings and a glyph index: the glyph moved, the index into the row
        // did not.
        math: row.math.clone(),
        region: row.region.clone(),
        // Relative, like `math`.
        inline_regions: row.inline_regions.clone(),
    }
}

/// Advance a row's source offsets by `delta` in place — the suffix half of
/// [`build_spliced`], where the rows are already owned and only need shifting,
/// not copying.
pub(super) fn shift_row_in_place(row: &mut VRow, delta: isize) {
    for g in &mut row.glyphs {
        g.src = (g.src as isize + delta) as usize;
    }
    row.end_src = (row.end_src as isize + delta) as usize;
    for o in &mut row.mark_ends {
        *o = (*o as isize + delta) as usize;
    }
}

/// Whether every source offset a block's rows carry falls inside the block's own
/// span — the precondition for reusing the block by a uniform offset shift. It
/// holds for well-formed blocks (their glyphs and row ends address bytes within
/// the block, synthetic glyphs point at the block start). It fails when a node
/// renders *outside* its block, which today means a malformed Markdown inline
/// node twig leaves with a degenerate `0..0` span: that content lands at a fixed
/// offset that doesn't move with the block. Such a block is re-rendered every
/// build instead of shifted, so the incremental map still matches a fresh one —
/// see [`build_cached`] and [`build_spliced`].
pub(super) fn rows_within(rows: &[VRow], span: &Range<usize>) -> bool {
    rows.iter().all(|r| {
        r.end_src >= span.start
            && r.end_src <= span.end
            && r.glyphs
                .iter()
                .all(|g| g.src >= span.start && g.src <= span.end)
    })
}
