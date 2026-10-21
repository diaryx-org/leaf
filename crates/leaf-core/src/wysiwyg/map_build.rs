//! Building a map: the surface a frontend describes, the reveal line, and the
//! full, cached and spliced builds.

use super::*;

/// What the surface a map is built for can paint, and how tall its pictures
/// came out — the things about a frontend that change the rows core lays
/// down, gathered from the frontend by [`crate::Doc`] and threaded into every
/// build. The defaults are the plain surface: nothing painted in a line, every
/// picture a one-row placeholder, which is what every test that passes
/// `Surface::default()` gets and what every build got before there was one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Surface {
    /// How many visual rows each block image reserves, keyed by its
    /// destination — the frontend's per-image height, set through
    /// [`crate::Doc::set_media_rows`] so [`Builder::block_media`] can size the
    /// placeholder without core doing any I/O. A destination absent from the
    /// map (or a `0`/`1` entry) reserves the bare one-row placeholder.
    pub media_rows: HashMap<String, usize>,
    /// The same for each display formula, keyed by its TeX verbatim (the
    /// [`MathMark::tex`] a frontend was handed) — set through
    /// [`crate::Doc::set_math_rows`] once the frontend has typeset and
    /// measured the picture.
    pub math_rows: HashMap<String, usize>,
    /// The same for each leaf directive, keyed by its [`DirectiveKey`] — set
    /// through [`crate::Doc::set_directive_rows`] by a frontend that draws a
    /// host's lines for the directive over reserved rows, as a terminal does.
    pub directive_rows: HashMap<DirectiveKey, usize>,
    /// Whether the surface can paint a picture *inside* a line of text, so an
    /// inline formula may render to one atom glyph it draws over
    /// ([`MathInfo`]). A pixel-laid-out frontend says yes; a terminal cannot
    /// composite an image over one cell, and leaves this off to keep an inline
    /// formula as the code-styled TeX it always showed. Off by default.
    pub inline_pictures: bool,
    /// Whether the surface sets a [`Baseline::Super`] glyph raised and
    /// smaller, rather than inline at body size. When it does, a footnote's
    /// label is set off from the prose by where it sits, and needs no
    /// brackets to be read as a reference: a reference draws as its bare
    /// raised label (`claim¹`), and a definition's marker as the same raised
    /// label, so the two still pair by eye. A terminal sets every glyph on the
    /// one baseline, where a bare `1` reads as `claim1`, a typo; it leaves
    /// this off and keeps the brackets. Off by default.
    pub raised_superscripts: bool,
    /// The region kinds the frontend edits itself
    /// ([`crate::Doc::set_owned_regions`]): [`MATH_REGION`], and
    /// [`FENCE_REGION`] with a language. A formula of an owned kind keeps its
    /// picture on the caret's line, and a fence of one is a placeholder row;
    /// each is published in [`VisualMap::regions`].
    pub owned_regions: BTreeSet<String>,
}

impl Surface {
    /// Whether the frontend edits regions of `kind` itself.
    pub fn owns(&self, kind: &str) -> bool {
        self.owned_regions.contains(kind)
    }
}

/// The one source line rendering its markup raw — the caret's, when the mode
/// or the content asks for it — and how much of the markup that means. See
/// [`crate::Doc::reveal_line`], which decides both.
///
/// Two grades because two things ask. Under
/// [`MarkupMode::Full`](crate::MarkupMode::Full) *every* delimiter on the line
/// comes back (`markup: true`). In the two hidden modes a formula still
/// reveals — its content is its TeX and not its picture, so hiding is not
/// enough — and the line is threaded through with `markup: false`: a math node
/// meeting it shows its source, and an emphasis meeting it hides its
/// asterisks as it always did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reveal {
    /// The source byte range of the line, newline excluded — empty but present
    /// on a blank line.
    pub line: Range<usize>,
    /// Whether ordinary inline markup reveals on it too, or only math.
    pub markup: bool,
}

impl Reveal {
    /// The caret's line under `MarkupMode::Full`: everything on it reveals.
    pub fn full(line: Range<usize>) -> Self {
        Reveal { line, markup: true }
    }

    /// The caret's line in a hidden mode, where only a formula reveals.
    pub fn math(line: Range<usize>) -> Self {
        Reveal {
            line,
            markup: false,
        }
    }

    /// Whether `span` meets this line — the test every arm that reveals runs.
    ///
    /// Touching at an endpoint counts: an emphasis ending exactly where the
    /// line does is on that line, and a zero-length reveal range (the caret
    /// alone on a blank line) still meets a node that starts there. The test
    /// is deliberately generous — the failure it avoids is revealing one
    /// delimiter of a pair while hiding the other, which looks like corruption
    /// rather than like markup.
    pub(super) fn meets(&self, span: &Range<usize>) -> bool {
        span.start <= self.line.end && self.line.start <= span.end
    }
}

/// Render the document to a [`VisualMap`]. `wrap` is the column budget for
/// word-wrapping (`Some` for the monospace TUI), or `None` to emit one row per
/// block — the GUI does its own proportional pixel wrapping over these rows.
/// Text and offsets come from the AST (`str` nodes carry the verbatim source
/// slice and an exact span), so the original source string isn't needed here.
pub fn build(
    nodes: &[FlatNode],
    source: &str,
    wrap: Option<usize>,
    preserve_soft: bool,
    surface: &Surface,
    reveal: Option<Reveal>,
) -> VisualMap {
    let Some(doc) = nodes.iter().position(|n| n.kind == Kind::Doc) else {
        return VisualMap::default();
    };
    let top = top_level(nodes, doc);
    let mut b = Builder {
        nodes,
        source,
        wrap: wrap.map(|w| w.max(8)),
        rows: Vec::new(),
        tables: Vec::new(),
        last_off: 0,
        stepped_over: 0,
        surface,
        break_glyph: Cell::new(' '),
        preserve_soft,
        reveal: reveal.clone(),
        pending_mark_ends: RefCell::new(Vec::new()),
        pending_math: RefCell::new(Vec::new()),
        pending_regions: RefCell::new(Vec::new()),
        saw_math: Cell::new(false),
        presentation: Presentation::default(),
        faces: RefCell::new(FaceTable::default()),
    };
    // The hidden frontmatter's end is the baseline for the leading and trailing
    // blank rows and for the caret floor — see [`hidden_prefix_end`].
    // `top_level` has already dropped every `metadata` child, so read it off
    // the arena.
    let hidden_end = hidden_prefix_end(source, metadata_end_of(nodes, doc));
    let last_drawn = b.top_blocks(&top, hidden_end);
    b.emit_trailing_blank_lines(last_drawn.unwrap_or(BlockClass::Paragraph), hidden_end);
    let content_start = floor_of(
        &b.rows,
        top.first().map_or(hidden_end, |&i| nodes[i].span.start),
    );
    let stops = collect_stops(&b.rows);
    let mark_ends = collect_mark_ends(&b.rows);
    label_media_boundaries(&mut b.rows);
    let code_blocks = code_block_spans(&b.rows);
    let media = media_spans(&b.rows);
    let directives = directive_spans(&b.rows);
    let math = math_spans(&b.rows);
    let regions = region_spans(&b.rows);
    VisualMap {
        rows: b.rows,
        content_start,
        stops,
        mark_ends,
        tables: b.tables,
        code_blocks,
        media,
        directives,
        math,
        regions,
        faces: b.faces.into_inner(),
        spelling: OnceLock::new(),
    }
}

/// Like [`build`], but reuses a persistent [`BlockCache`] so an edit re-renders
/// only the top-level blocks whose source bytes changed *and* marshals only
/// those blocks from twig instead of the whole arena.
///
/// `top` is the document's top-level blocks — twig's `child_spans` of the doc
/// root: `(node_id, kind, span)` for each, in order. `fetch_subtree(node_id)`
/// marshals one block's subtree (local-indexed, root at 0) and is called *only*
/// for a block that missed the cache, i.e. one that actually changed. So a
/// keystroke marshals one small subtree, not ~20k nodes. The result is
/// byte-for-byte identical to [`build`] on the same document (the
/// `build_cached_matches_build` test pins this); [`build`] stays the cache-free,
/// whole-arena reference. This is the entry point [`crate::Doc`] uses.
// One builder, and every one of these is a distinct input to the same layout
// pass — a struct of them would be built at the one call site and unpacked
// here, which is the same arguments with an extra name in the way.
#[allow(clippy::too_many_arguments)]
pub fn build_cached(
    top: &[QueryMatch],
    source: &str,
    wrap: Option<usize>,
    preserve_soft: bool,
    surface: &Surface,
    reveal: Option<Reveal>,
    cache: &mut BlockCache,
    mut fetch_subtree: impl FnMut(u32) -> Vec<FlatNode>,
) -> VisualMap {
    let wrap = wrap.map(|w| w.max(8));

    // Wrapping is a function of the width, so a width change makes every cached
    // row's wrap wrong: start the cache over.
    if cache.wrap != Some(wrap) {
        cache.entries.clear();
        cache.wrap = Some(wrap);
    }
    cache.generation = cache.generation.wrapping_add(1);

    // Frontmatter (a leading `metadata` block) is document metadata, not prose:
    // hidden in the rich view exactly as [`Builder::blocks`] skips it.
    let blocks: Vec<&QueryMatch> = top.iter().filter(|m| m.kind != Kind::Metadata).collect();

    // The outer builder only accumulates rows/tables and spells block boundaries
    // — both a function of the source and `last_off`, never of a node array — so
    // it carries an empty `nodes`. Each changed block is rendered by a *fresh*
    // builder over that block's subtree.
    let mut b = Builder {
        nodes: &[],
        source,
        wrap,
        rows: Vec::new(),
        tables: Vec::new(),
        last_off: 0,
        stepped_over: 0,
        surface,
        break_glyph: Cell::new(' '),
        preserve_soft,
        reveal: reveal.clone(),
        pending_mark_ends: RefCell::new(Vec::new()),
        pending_math: RefCell::new(Vec::new()),
        pending_regions: RefCell::new(Vec::new()),
        saw_math: Cell::new(false),
        presentation: Presentation::default(),
        faces: RefCell::new(FaceTable::default()),
    };

    // Record the per-block row decomposition as we go, so a later
    // [`build_spliced`] can patch one block without rebuilding the map.
    let mut layout_blocks: Vec<BlockLayout> = Vec::with_capacity(blocks.len());
    // The document's face table, assembled block by block: from the walk on a
    // miss, from what the entry stored on a hit. The outer builder walks no
    // attributes of its own (it spells boundaries), so it never adds to it.
    let mut faces = FaceTable::default();
    let mut all_shift_safe = true;
    // The class of the last block that drew anything: what the next separator
    // closes, and what the trailing blank lines close at the end. A hidden block
    // (a comment) never becomes it — see [`Builder::block_or_hidden`], whose
    // step-over this loop repeats for the incremental walk.
    let mut above: Option<BlockClass> = None;
    let hidden_end = hidden_prefix_end(
        source,
        top.iter()
            .filter(|m| m.kind == Kind::Metadata)
            .map(|m| m.span.end)
            .next_back(),
    );
    for block in &blocks {
        let start = block.span.start;
        let before_sep = b.rows.len();
        // This walker has no node arena at all (see the `nodes: &[]` above),
        // but a top-level query match carries its kind — the same string
        // `BlockClass::from_node_kind` classifies for the whole-arena walk, so
        // the incremental and full builds label a boundary identically.
        let below = BlockClass::from_node_kind(&block.kind);
        if let Some(above) = above {
            b.emit_separators_before(start, &[], true, Boundary { above, below });
        } else {
            let from = b.leading_from(hidden_end);
            b.emit_leading_blank_lines(from, start, below);
        }
        let after_sep = b.rows.len();
        let bytes = block_bytes(source, &block.span);
        let hash = block_hash(bytes);
        // How this block meets the reveal line, if at all — part of its cache
        // key, since the same bytes render differently on the caret's line.
        let rkey = reveal_key(&reveal, &block.span);

        // Hit: clone the block's rows shifted to its current offset and restore
        // the (shifted) `last_off` so the next separator lands right — no marshal.
        // Only shift-safe blocks are ever cached, so a hit is safe by construction.
        let mut has_math = false;
        if let Some(hit) = cache.reuse(hash, &block.kind, bytes, &rkey) {
            faces.merge(&hit.faces);
            has_math = hit.has_math;
            let delta = start as isize - hit.built_start as isize;
            for row in &hit.rows {
                b.rows.push(shift_row(row, delta));
            }
            b.last_off = (hit.last_off as isize + delta) as usize;
            // `0` is a block that stepped over nothing, and no answer to shift.
            if hit.stepped_over > 0 {
                let stepped = (hit.stepped_over as isize + delta) as usize;
                b.stepped_over = b.stepped_over.max(stepped);
            }
        } else {
            // Miss: marshal just this block's subtree and render it. A subtree is
            // self-contained with local ids (root at 0) and absolute spans, so a
            // fresh builder over it produces the same rows the whole-arena path
            // would. An empty subtree (twig couldn't hand it back) renders nothing.
            let subtree = fetch_subtree(block.node_id);
            if !subtree.is_empty() {
                let mut sub = Builder {
                    nodes: &subtree,
                    source,
                    wrap,
                    rows: Vec::new(),
                    tables: Vec::new(),
                    last_off: 0,
                    stepped_over: 0,
                    surface,
                    break_glyph: Cell::new(' '),
                    preserve_soft,
                    reveal: reveal.clone(),
                    pending_mark_ends: RefCell::new(Vec::new()),
                    pending_math: RefCell::new(Vec::new()),
                    pending_regions: RefCell::new(Vec::new()),
                    saw_math: Cell::new(false),
                    presentation: Presentation::default(),
                    faces: RefCell::new(FaceTable::default()),
                };
                sub.block(0, &[], &[]);
                // A block that drew nothing is stepped over, not stood on: its
                // `last_off` is its own end, so the separator after it counts
                // from there. The sub-builder started at 0 and never moved, and
                // 0 is where the next separator would otherwise count from —
                // every line of the document, as a blank row each.
                let last_off = if sub.rows.is_empty() {
                    block.span.end
                } else {
                    sub.last_off
                };
                let stepped_over = sub.stepped_over;
                b.stepped_over = b.stepped_over.max(stepped_over);
                // The names this block's glyph ids stand for. They go into the
                // document's table *and* into the cache entry, because a hit
                // re-emits these rows without walking an attribute again.
                let block_faces = sub.faces.into_inner();
                faces.merge(&block_faces);
                has_math = sub.saw_math.get();
                // Cache only a block that is table-free AND renders inside its own
                // span: those two are the conditions for reuse-by-shift to be
                // correct. A block failing either is re-rendered every build (a
                // fresh render always matches a fresh whole-document build).
                if sub.tables.is_empty() {
                    if rows_within(&sub.rows, &block.span) {
                        cache.store(
                            hash,
                            &block.kind,
                            bytes,
                            start,
                            sub.rows.clone(),
                            last_off,
                            stepped_over,
                            rkey,
                            has_math,
                            block_faces,
                        );
                    }
                    b.rows.extend(sub.rows);
                } else {
                    // A table block is never cached; rebase its row-index
                    // bookkeeping onto the combined row vector and append.
                    let base = b.rows.len();
                    for t in &mut sub.tables {
                        t.rows_span = (t.rows_span.start + base)..(t.rows_span.end + base);
                    }
                    b.rows.extend(sub.rows);
                    b.tables.extend(sub.tables);
                }
                b.last_off = last_off;
            }
        }
        let content_rows = b.rows.len() - after_sep;
        let sep_rows = if content_rows == 0 {
            // Hidden: take back the separator drawn for it, so what stands
            // either side meets across one boundary. Its layout entry stays, at
            // no rows, so the splice arithmetic still counts one entry per block.
            b.rows.truncate(before_sep);
            // A cache hit restored the stored `last_off` above; an empty subtree
            // (twig couldn't hand it back) restored nothing. Either way the walk
            // stands past the block.
            b.last_off = b.last_off.max(block.span.end);
            b.stepped_over = b.stepped_over.max(block.span.end);
            0
        } else {
            above = Some(BlockClass::from_node_kind(&block.kind));
            all_shift_safe &= rows_within(&b.rows[after_sep..], &block.span);
            after_sep - before_sep
        };
        layout_blocks.push(BlockLayout {
            span: block.span.clone(),
            kind: block.kind.clone(),
            sep_rows,
            content_rows,
            has_math,
        });
    }

    let before_trailing = b.rows.len();
    b.emit_trailing_blank_lines(above.unwrap_or(BlockClass::Paragraph), hidden_end);
    let trailing_rows = b.rows.len() - before_trailing;

    // Evict every entry no block reused this build, so the cache tracks the
    // current document instead of growing without bound over a session.
    let g = cache.generation;
    cache.entries.retain(|_, bucket| {
        bucket.retain(|e| e.generation == g);
        !bucket.is_empty()
    });

    cache.layout = Layout {
        blocks: layout_blocks,
        trailing_rows,
        built_len: source.len(),
        has_tables: !b.tables.is_empty(),
        all_shift_safe,
        reveal: reveal.clone(),
    };

    // The first rendered offset is the first non-metadata block's start — the
    // analogue of [`first_content_offset`] for the top-level list. With nothing
    // but frontmatter it's the end of that frontmatter, and 0 for an empty
    // document ([`hidden_prefix_end`]).
    let content_start = floor_of(&b.rows, blocks.first().map_or(hidden_end, |m| m.span.start));
    let stops = collect_stops(&b.rows);
    let mark_ends = collect_mark_ends(&b.rows);
    label_media_boundaries(&mut b.rows);
    let code_blocks = code_block_spans(&b.rows);
    let media = media_spans(&b.rows);
    let directives = directive_spans(&b.rows);
    let math = math_spans(&b.rows);
    let regions = region_spans(&b.rows);
    VisualMap {
        rows: b.rows,
        content_start,
        stops,
        mark_ends,
        tables: b.tables,
        code_blocks,
        media,
        directives,
        math,
        regions,
        faces,
        spelling: OnceLock::new(),
    }
}

/// The fast path for a single-block edit: patch the previous [`VisualMap`] in
/// place rather than reassembling it. Returns `Some(new_map)` when it applies,
/// or `None` to tell the caller to fall back to [`build_cached`] (always
/// correct). Consumes `prev` either way — on `None` the caller rebuilds from
/// scratch and doesn't need it.
///
/// It applies only when `dirty` (twig's dirty byte range) falls inside exactly
/// one top-level block AND the block structure around it is unchanged — verified
/// by matching the new `top` list against the previous [`Layout`] block for
/// block: kinds unchanged, spans before the edit identical, spans after it
/// shifted by the byte delta, count unchanged. Any deviation — a block split or
/// merged, a fence opened to swallow later blocks, a table anywhere, a
/// multi-block edit — fails the match and returns `None`. That check is what
/// makes the byte-range trustworthy: twig's dirty range is exact about *bytes*
/// but silent about *reparse*, and the structural match catches the reparse
/// effects it can't see.
///
/// When it applies, the unchanged prefix rows move verbatim, the suffix rows
/// shift by the delta *in place* (integer adds, no glyph copy), and only the one
/// dirty block is re-marshalled and re-rendered; stops splice the same way by
/// offset. So the cost is O(rows after the edit), and nothing before the edit is
/// touched. The hash-keyed entry cache is left alone — a later [`build_cached`]
/// will miss on the changed block, re-render it, and evict the stale entry, so
/// chained splices neither corrupt nor grow it.
// One builder, and every one of these is a distinct input to the same layout
// pass — a struct of them would be built at the one call site and unpacked
// here, which is the same arguments with an extra name in the way.
#[allow(clippy::too_many_arguments)]
pub fn build_spliced(
    prev: VisualMap,
    source: &str,
    wrap: Option<usize>,
    preserve_soft: bool,
    top: &[QueryMatch],
    dirty: Range<usize>,
    surface: &Surface,
    reveal: Option<Reveal>,
    cache: &mut BlockCache,
    mut fetch_subtree: impl FnMut(u32) -> Vec<FlatNode>,
) -> Option<VisualMap> {
    let wrap = wrap.map(|w| w.max(8));
    // A width change invalidates every cached row — a full rebuild's job.
    if cache.wrap != Some(wrap) {
        return None;
    }
    // So does a moved reveal line, and for the same reason: this path reuses
    // every row outside the dirty block, and those rows encode which line was
    // showing its raw markup when they were built. Typing almost always moves
    // the caret, so under `MarkupMode::Full` this bails to `build_cached` on
    // most keystrokes — still block-cached, so only the edited block and the
    // revealed one actually re-render.
    if cache.layout.reveal != reveal {
        return None;
    }
    // Take the previous layout; on any bail below the caller rebuilds it (and the
    // map) via `build_cached`, so leaving it empty is fine. A table or a block
    // that renders outside its span (a degenerate inline span) makes shifting
    // unsound, so those force the full-rebuild path.
    let prev_layout = std::mem::take(&mut cache.layout);
    if prev_layout.built_len == 0 || prev_layout.has_tables || !prev_layout.all_shift_safe {
        return None;
    }
    // The layout addresses `prev` by row index, so it is only usable against the
    // map it was built from. A frontend is free to hold the map it was handed and
    // present it differently — leaf-ratatui splices blank filler rows under an
    // oversized heading so the raster has somewhere to stand — and if one of those
    // comes back here the row arithmetic below lands on the wrong rows: the
    // re-rendered block is laid over a filler and the rows it really occupied
    // survive into the suffix, stranding a stale copy of the edited line and
    // pushing everything after it one row down, once per keystroke. A row count
    // that doesn't match what this layout describes is the tell, and the honest
    // answer is the full rebuild.
    let described_rows = prev_layout
        .blocks
        .iter()
        .map(|pl| pl.sep_rows + pl.content_rows)
        .sum::<usize>()
        + prev_layout.trailing_rows;
    if described_rows != prev.rows.len() {
        return None;
    }

    let blocks: Vec<&QueryMatch> = top.iter().filter(|m| m.kind != Kind::Metadata).collect();
    if blocks.is_empty() || blocks.len() != prev_layout.blocks.len() {
        return None;
    }
    let delta = source.len() as isize - prev_layout.built_len as isize;

    // The single block whose NEW span contains the whole dirty range. A dirty
    // range straddling a block boundary (or a separator) finds none → bail.
    let k = blocks
        .iter()
        .position(|m| m.span.start <= dirty.start && dirty.end <= m.span.end)?;

    // Structural match: every OTHER block is unchanged — same kind throughout,
    // span identical before the edit and shifted by `delta` after it. A mismatch
    // means the reparse reshaped the block structure, which only a full rebuild
    // renders correctly.
    for (i, (m, pl)) in blocks.iter().zip(&prev_layout.blocks).enumerate() {
        if m.kind != pl.kind {
            return None;
        }
        if i == k {
            continue;
        }
        let want = if i < k {
            pl.span.clone()
        } else {
            (pl.span.start as isize + delta) as usize..(pl.span.end as isize + delta) as usize
        };
        if m.span != want {
            return None;
        }
    }
    // The dirty block itself: start unchanged (the edit is inside it, past its
    // start), end moved by exactly the delta.
    let pk_start = prev_layout.blocks[k].span.start;
    let pk_end = prev_layout.blocks[k].span.end;
    let pk_sep = prev_layout.blocks[k].sep_rows;
    let pk_content = prev_layout.blocks[k].content_rows;
    if blocks[k].span.start != pk_start || blocks[k].span.end != (pk_end as isize + delta) as usize
    {
        return None;
    }

    // Re-render the dirty block from its subtree. A table makes the splice
    // bookkeeping unsafe, so bail if one appears.
    let subtree = fetch_subtree(blocks[k].node_id);
    if subtree.is_empty() {
        return None;
    }
    let mut sub = Builder {
        nodes: &subtree,
        source,
        wrap,
        rows: Vec::new(),
        tables: Vec::new(),
        last_off: 0,
        stepped_over: 0,
        surface,
        break_glyph: Cell::new(' '),
        preserve_soft,
        reveal: reveal.clone(),
        pending_mark_ends: RefCell::new(Vec::new()),
        pending_math: RefCell::new(Vec::new()),
        pending_regions: RefCell::new(Vec::new()),
        saw_math: Cell::new(false),
        presentation: Presentation::default(),
        faces: RefCell::new(FaceTable::default()),
    };
    sub.block(0, &[], &[]);
    // A table, or content that renders outside the block's span (a degenerate
    // inline span), makes the shift bookkeeping unsound — fall back.
    if !sub.tables.is_empty() || !rows_within(&sub.rows, &blocks[k].span) {
        return None;
    }
    // The face table starts from the previous map's, because every row this
    // path keeps was built against it and its glyphs' ids still mean what they
    // meant. The re-rendered block adds whatever it met. An id the edit took
    // the last glyph of stays in the table, naming nothing — the price of not
    // walking the rows this path exists to avoid walking.
    let mut faces = prev.faces;
    faces.merge(&sub.faces.into_inner());
    let sub_saw_math = sub.saw_math.get();
    let new_content = sub.rows;
    let new_content_len = new_content.len();
    let new_stops = collect_stops(&new_content);
    let new_mark_ends = collect_mark_ends(&new_content);

    // Row span of the dirty block's CONTENT. Its leading separator stays in the
    // prefix: the gap before block k is unchanged, since k's start didn't move.
    let content_start_row: usize = prev_layout.blocks[..k]
        .iter()
        .map(|pl| pl.sep_rows + pl.content_rows)
        .sum::<usize>()
        + pk_sep;
    let content_end_row = content_start_row + pk_content;

    // Splice rows: [prefix | new content | suffix + delta]. The prefix moves
    // untouched; the suffix shifts in place — integer adds, no glyph copy.
    let mut rows = prev.rows;
    let mut suffix = rows.split_off(content_end_row);
    rows.truncate(content_start_row);
    for row in &mut suffix {
        shift_row_in_place(row, delta);
    }
    rows.reserve(new_content_len + suffix.len());
    rows.extend(new_content);
    rows.extend(suffix);

    // Splice stops by offset. The old dirty block covered `[pk_start, pk_end]`:
    // prefix stops fall below it, suffix stops above it (shift by delta), the new
    // content supplies the middle. The three ranges stay disjoint and ascending,
    // so the result needs no re-sort.
    let p1 = prev.stops.partition_point(|&s| s < pk_start);
    let p2 = prev.stops.partition_point(|&s| s <= pk_end);
    let mut stops = Vec::with_capacity(p1 + new_stops.len() + (prev.stops.len() - p2));
    stops.extend_from_slice(&prev.stops[..p1]);
    stops.extend(new_stops);
    for &s in &prev.stops[p2..] {
        stops.push((s as isize + delta) as usize);
    }
    // The mark ends splice the same way: they are offsets in the same
    // coordinates, cut at the same block.
    let m1 = prev.mark_ends.partition_point(|&s| s < pk_start);
    let m2 = prev.mark_ends.partition_point(|&s| s <= pk_end);
    let mut mark_ends = Vec::with_capacity(m1 + new_mark_ends.len() + (prev.mark_ends.len() - m2));
    mark_ends.extend_from_slice(&prev.mark_ends[..m1]);
    mark_ends.extend(new_mark_ends);
    for &s in &prev.mark_ends[m2..] {
        mark_ends.push((s as isize + delta) as usize);
    }

    // Record the patched layout for the next splice: spans move to the new
    // coordinates, and the dirty block takes its new content-row count.
    let mut new_blocks = prev_layout.blocks;
    for (pl, m) in new_blocks.iter_mut().zip(&blocks) {
        pl.span = m.span.clone();
    }
    new_blocks[k].content_rows = new_content_len;
    new_blocks[k].has_math = sub_saw_math;
    cache.layout = Layout {
        blocks: new_blocks,
        trailing_rows: prev_layout.trailing_rows,
        built_len: source.len(),
        has_tables: false,
        // Every prefix/suffix block was shift-safe last build (we bailed
        // otherwise) and the re-rendered block was just checked, so the patched
        // document is still entirely shift-safe.
        all_shift_safe: true,
        reveal,
    };

    label_media_boundaries(&mut rows);
    let code_blocks = code_block_spans(&rows);
    let media = media_spans(&rows);
    let directives = directive_spans(&rows);
    let math = math_spans(&rows);
    let regions = region_spans(&rows);
    Some(VisualMap {
        rows,
        // The edit was inside one block, so no block moved its start and the
        // lines above the first are the ones the floor was taken from.
        content_start: prev.content_start,
        stops,
        mark_ends,
        tables: Vec::new(),
        code_blocks,
        media,
        directives,
        math,
        regions,
        faces,
        spelling: OnceLock::new(),
    })
}
