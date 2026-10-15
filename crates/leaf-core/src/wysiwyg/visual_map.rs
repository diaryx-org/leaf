//! The visual map's queries: offset ⇄ (row, col), caret stops, motion, and
//! the spans a frontend paints over.

use super::*;

/// The rendered document plus the offset⇄position mapping the caret rides on.
#[derive(Clone, Default)]
pub struct VisualMap {
    /// The document's **default monospace rendering** — one [`VRow`] of glyphs
    /// per visual line, tables spelled with box-drawing borders (`│ ─ ┌┬┐…`) and
    /// cells padded to whole character-cell columns. Any monospace surface can
    /// draw these verbatim, so a consumer gets a working view for free: the TUI
    /// paints them as-is, and a five-line plain-text dump would too.
    ///
    /// It's a *default*, not the only truth. A frontend with its own geometry —
    /// a proportional GUI — lays text out in its own units, and for a table
    /// skips the box-drawn rows named by [`TableInfo::rows_span`] and draws from
    /// the structural [`TableInfo`] instead. The box glyphs live here rather than
    /// in a frontend precisely because they *are* a renderable default: unlike a
    /// colour (a role each surface must map to its own palette — see
    /// [`crate::style`]), `┌─┐` is finished text that needs no interpretation.
    pub rows: Vec<VRow>,
    /// The first source offset that is actually rendered — the caret floor for
    /// the WYSIWYG view. Non-zero when a leading `metadata` block (YAML/TOML
    /// frontmatter) is skipped: the frontmatter is preserved in the source and
    /// editable in the source view, but hidden and unreachable here, so the
    /// caret and selection can't wander into it (and copy won't grab it).
    pub content_start: usize,
    /// Every offset the caret may rest at, ascending and deduplicated: each
    /// row's stop glyphs plus the row's own end (the "after the last character"
    /// spot every line needs). Decoration contributes nothing.
    ///
    /// Left/Right read this instead of walking the grid, because the grid isn't
    /// laid out in offset order: a table with wrapped cells puts column 1's
    /// second line *below* column 2's first, so "the next stop rightward" and
    /// "the next stop in the document" part ways. Following the document is what
    /// a caret means — and on every row that *is* in order the two agree anyway,
    /// so nothing else has to change.
    pub(super) stops: Vec<usize>,
    /// The caret's second home at the end of every hidden inline mark: the
    /// offset where the mark's content ends, one byte before its closing
    /// delimiter — ascending and deduplicated, from every row's
    /// [`VRow::mark_ends`].
    ///
    /// With delimiters hidden, `**bold** tail` draws one spot after the `d`
    /// and the source has two offsets for it: the content end (inside the
    /// mark, where typing extends the bold) and the byte past the `**` (where
    /// typing leaves it). Only the second is a glyph's offset, so only it was
    /// a stop, and a caret asked to rest at the first was snapped a whole
    /// character back onto the `d` — a drag over `bold` came back one letter
    /// short. The delete and backspace paths already settle the caret on the
    /// content end as its natural home there
    /// ([`crate::Doc::settle_inside_close_delims`]); this makes it one the
    /// caret can be placed at and step onto too.
    ///
    /// Kept apart from [`stops`](Self::stops) rather than merged in, because
    /// the two lists answer different questions. A stop with no glyph is
    /// invisible to a walk that pairs stops with characters — a system text
    /// input counting `position(from:offset:)` steps against the text it was
    /// shown would drift a character at every mark — and to word motion, which
    /// classifies a stop by the source byte under it (a `*`). So
    /// [`stop_after`](Self::stop_after) and its kin walk the glyph stops alone,
    /// and only the places a caret *rests* — snapping, resting checks, and
    /// Left/Right — read both.
    pub(super) mark_ends: Vec<usize>,
    /// Every table in the document, in order, described structurally rather than
    /// drawn — see [`TableInfo`] for why both exist.
    pub tables: Vec<TableInfo>,
    /// Every fenced/indented code block, in order, as the range of [`rows`] it
    /// occupies — a frontend draws one bordered, tinted box around each and
    /// scrolls it horizontally rather than wrapping. Derived from the per-row
    /// [`VRow::code`] flag once the rows are final (so it survives incremental
    /// row reuse), the same way [`collect_stops`] derives the stop table.
    ///
    /// [`rows`]: VisualMap::rows
    pub code_blocks: Vec<CodeBlockInfo>,
    /// Every block-level image in the document, in order — one per placeholder
    /// row a frontend replaces with a real picture. Derived from the per-row
    /// [`VRow::media`] mark once the rows are final (so it survives incremental
    /// row reuse), the same way [`code_blocks`](VisualMap::code_blocks) is
    /// derived from [`VRow::code`].
    pub media: Vec<MediaInfo>,
    /// Every **leaf** directive in the document, in order — one per placeholder
    /// row a frontend may replace with whatever the host app's vocabulary makes
    /// of it. Derived from the per-row [`VRow::leaf_directive`] mark once the
    /// rows are final, exactly as [`media`](VisualMap::media) is.
    pub directives: Vec<DirectiveInfo>,
    /// Every formula standing as a picture in this map, in row order — each
    /// inline atom and each display block's placeholder. A frontend that
    /// paints pictures typesets each one's TeX and draws it over the glyph or
    /// the rows named. Derived from the per-row [`VRow::math`] marks once the
    /// rows are final, exactly as [`media`](VisualMap::media) is.
    ///
    /// A formula on the revealed line is not here: there it is its own TeX,
    /// drawn as code, and nothing stands in for it.
    pub math: Vec<MathInfo>,
    /// Every region the frontend edits itself, in row order — each owned
    /// formula, and each owned fence's placeholder row. Derived from the rows
    /// once they are final, as [`math`](VisualMap::math) is.
    pub regions: Vec<RegionInfo>,
    /// Every named font family this map's glyphs are set in, by the
    /// [`FaceId`] they carry — the side table that lets [`Style`] stay `Copy`
    /// while a family name stays a `String`.
    ///
    /// Not derived from the rows the way [`code_blocks`](Self::code_blocks) is,
    /// because the name is not on the rows: it is interned as the walker meets
    /// the attribute. So each of the three build paths assembles it from what
    /// it actually walked — a fresh build from its own walk, a cached build
    /// from each block's walk or the names its cache entry stored, a splice
    /// from the previous map's table plus the one block it re-rendered. A
    /// [`FaceId`] is derived from the name rather than being an index, which is
    /// what makes those three agree glyph for glyph; see the type's note.
    pub(super) faces: FaceTable,
    /// The visible text tabulated over the stops — see [`Spelling`]. Derived
    /// on the first lookup that asks for it rather than by the build paths,
    /// since a map that is only ever drawn never needs it, and a splice
    /// would otherwise have to patch it the way it patches the stops.
    pub(super) spelling: OnceLock<Spelling>,
}

/// The visible text ([`VisualMap::visible_text`]) as a table over the stops:
/// for stop `i`, the character the text spells it with — `None` for the
/// `'\n'` of a stop that draws no glyph — and the UTF-16 length of the text
/// before it. A UTF-16 index and a stop then find each other by binary
/// search, where they used to find each other by walking every character of
/// the document from the top: a frontend converts an `NSRange` end per
/// selection change and per misspelled word, and each conversion was a whole
/// document's work.
///
/// Built once per map and never maintained. The stops are private and fixed
/// for the map's life; the rows only lend the character each stop draws, and
/// a frontend that reshapes the rows after the build (the terminal inserts
/// blank filler rows) neither changes those characters nor asks this.
#[derive(Clone, Default)]
pub(super) struct Spelling {
    /// Parallel to [`VisualMap::stops`].
    chars: Vec<Option<char>>,
    /// One longer than `chars`: `utf16[i]` is the UTF-16 length of the text
    /// before stop `i`, and the last entry the length of the whole text.
    utf16: Vec<usize>,
}

impl VisualMap {
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    /// The family name a glyph's [`FaceRef::Named`] stands for, or `None` for
    /// an id from another map — which a frontend draws in the theme's body
    /// face, as it draws a family it cannot resolve.
    pub fn face_name(&self, id: FaceId) -> Option<&str> {
        self.faces.name(id)
    }

    /// Every named family this map draws — how a frontend warms a font cache
    /// before it lays a frame out. See [`faces`](Self::faces).
    ///
    /// [`faces`]: VisualMap::faces
    pub fn faces(&self) -> &FaceTable {
        &self.faces
    }

    /// The width of `row` in display columns — the rightmost column its caret
    /// can occupy, and so what a goal column is clamped to on the way in.
    pub fn row_width(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, |r| r.width())
    }

    /// The screen `(row, col)` for a source offset — where to draw the caret:
    /// the *nearest* stop at or past `off`. Snaps a hidden offset (inside a
    /// delimiter) to the next visible glyph, and never resolves onto decoration
    /// (a table border, a cell's padding), which is drawn but holds no caret.
    ///
    /// "Nearest" rather than "the first one found" because a table's wrapped
    /// cells put rows slightly out of offset order: scanning top to bottom, the
    /// second line of column 1 comes *after* the first line of column 2 but
    /// holds smaller offsets. Where rows are in order the two rules agree.
    ///
    /// A soft wrap is the one place two rows want the same offset: the row above
    /// ends where the row below opens, the space the wrap ate being drawn on the
    /// row above and the offset past it being the row below's first character.
    /// It resolves *downstream*, to the row that character is on — the row
    /// above's last column is a phantom, a place the caret can be drawn but
    /// never sent, and resolving upstream into it is what pinned Down at the
    /// first wrap of a paragraph: it aimed at the row below's column 0, landed
    /// on the offset it already had, and read that back as the row above's end.
    ///
    /// The walk is over the rows, not their text: a row before `off` is
    /// dismissed by looking at where it opens and where it reaches — its first
    /// stop and its last — rather than at every glyph between, so the cost of
    /// a lookup mid-document is a few hundred rows' worth of two glyphs each,
    /// and the one row that holds the answer is the only one read through.
    /// It used to read every row through, and worse: the row's end candidate
    /// was built eagerly, and building it measured the row's width, which
    /// segments the row's whole text into grapheme clusters — so every row
    /// before `off` paid a full cluster walk to learn it held nothing, and a
    /// document of long paragraphs paid milliseconds per caret placement.
    ///
    /// Not a binary search, though the rows' opening stops ascend. A table's
    /// wrapped cells put several rows before the one an offset opens on that
    /// can still hold it, and `end_src` does not ascend across those rows at
    /// all, so a search would need a prefix table over the rows — and
    /// [`rows`](Self::rows) is public, reshaped by a frontend after the build
    /// (the terminal inserts filler rows under a heading), which is exactly
    /// what a table over it could not survive. The walk needs no table.
    pub fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        // (src, row, the glyph — or `None` for the caret past the row's end)
        let mut best: Option<(usize, usize, Option<usize>)> = None;
        for (r, row) in self.rows.iter().enumerate() {
            if row.decoration {
                continue;
            }
            // A row's *first* stop never decreases from one row to the next —
            // true even across a table's wrapped cells, since a cell's lines run
            // downward. So once a row opens past the best found so far, no later
            // row can beat it and the scan stays proportional to `off`.
            let open = row
                .glyphs
                .iter()
                .find(|g| g.stop)
                .map_or(row.end_src, |g| g.src);
            if best.is_some_and(|b| open > b.0) {
                break;
            }
            // Offsets ascend *within* a row, so its last stop or its end is as
            // far as it reaches: a row that reaches short of `off` has nothing
            // to offer, and says so from its two ends.
            let reach = row
                .glyphs
                .iter()
                .rev()
                .find(|g| g.stop)
                .map_or(row.end_src, |g| g.src.max(row.end_src));
            if reach < off {
                continue;
            }
            // And so its first stop at or past `off` is the best it has.
            let cand = row
                .glyphs
                .iter()
                .enumerate()
                .find(|(_, g)| g.stop && g.src >= off)
                .map(|(i, g)| (g.src, r, Some(i)))
                .or_else(|| (row.end_src >= off).then_some((row.end_src, r, None)));
            // `<=`, so a tie goes to the later row: the only offset two rows
            // both hold is a wrap boundary, and it belongs to the row below.
            if let Some(c) = cand
                && best.is_none_or(|b| c.0 <= b.0)
            {
                best = Some(c);
            }
        }
        match best {
            Some((_, r, Some(i))) => (r, self.rows[r].col_of_glyph(i)),
            Some((_, r, None)) => (r, self.rows[r].width()),
            None => {
                let r = self.last_stop_row();
                (r, self.row_width(r))
            }
        }
    }

    /// The rows a source range occupies, inclusive: `(first, last)`.
    ///
    /// A *different question* from [`pos_of_offset`](Self::pos_of_offset), which
    /// is why it can't be spelled with two calls to it. That one answers "where
    /// does the caret go", and for a caret its forward snap is right — an offset
    /// inside a hidden delimiter has no column of its own, so the caret belongs
    /// at the next visible glyph, wherever that turns out to be. This one asks
    /// "which rows does this block cover", and there the snap is a trap: a
    /// footnote whose body *ends* in a link (`[^2]: [title](url)`) has a last
    /// byte inside the hidden destination, so `pos_of_offset(end - 1)` walked
    /// clean off the note's row and landed on the next note's — and a peek
    /// slicing `first..=last` out of the frame drew two notes where the reader
    /// asked for one. Every block ending in a link, an image, or any trailing
    /// hidden markup had the same fault; only a block ending in visible text
    /// (which is what the tests happened to use) did not.
    ///
    /// `row.end_src` is no help either: it is where the *rendered* text of a row
    /// ends, not how far into the source the block reaches, and redefining it
    /// would move every end-of-line caret.
    ///
    /// So the last row is found by asking which rows *open* before the range
    /// does, rather than by mapping its last byte: a row belongs to the range
    /// when its first caret stop lies before `range.end`. Decoration is skipped
    /// (a drawn gap between blocks is not part of either), and the answer is
    /// never shorter than one row — a range whose every byte is hidden still
    /// covers the row it started on.
    pub fn row_range_for(&self, range: Range<usize>) -> (usize, usize) {
        if self.rows.is_empty() {
            return (0, 0);
        }
        let first = self.pos_of_offset(range.start).0;
        let mut last = first;
        for (r, row) in self.rows.iter().enumerate().skip(first) {
            if row.decoration {
                continue;
            }
            let open = row
                .glyphs
                .iter()
                .find(|g| g.stop)
                .map_or(row.end_src, |g| g.src);
            if open >= range.end {
                // A row's first stop never decreases from one row to the next —
                // the invariant `pos_of_offset` breaks on, true even across a
                // table's wrapped cells — so nothing below can be in range.
                break;
            }
            last = r;
        }
        (first, last)
    }

    /// The source offset of the task checkbox drawn at `(row, col)`, or `None`
    /// when that cell holds no box — the hit-test a frontend runs on a click
    /// before treating it as a tick rather than a caret placement.
    ///
    /// Only the box's own cells answer. Clicking an item's *text* places the
    /// caret like any other click, so the box is a target aimed at rather than
    /// something tripped over while editing — which is also why this is a
    /// separate question from [`offset_of_pos`](Self::offset_of_pos) instead of
    /// a flag on the offset it returns.
    pub fn task_box_at(&self, row: usize, col: usize) -> Option<usize> {
        let r = self.rows.get(row)?;
        self.task_box_at_glyph(row, r.glyph_at_col(col)?)
    }

    /// [`task_box_at`](Self::task_box_at) keyed by glyph index rather than
    /// display column — for a frontend that shapes its own rows (the GUI) and so
    /// resolves a click to a glyph before it ever has a column.
    pub fn task_box_at_glyph(&self, row: usize, glyph: usize) -> Option<usize> {
        let r = self.rows.get(row)?;
        r.task?;
        let g = r.glyphs.get(glyph)?;
        (g.style.role == Role::ListMarker).then_some(g.src)
    }

    /// The source offset for a screen `(row, col)` — where a click or a
    /// visual-space move lands the caret. Clicking decoration maps through its
    /// `src`, which points at the text it decorates, so a click on a border or
    /// on a cell's padding lands in that cell.
    ///
    /// The inverse of [`pos_of_offset`](Self::pos_of_offset), which it has to
    /// agree with: `col` is a display column, and the one it names may be the
    /// far cell of a wide glyph — [`VRow::glyph_at_col`] is where that lands.
    pub fn offset_of_pos(&self, row: usize, col: usize) -> usize {
        let Some(r) = self.rows.get(row) else {
            // A click or drag below the last row — a short document with empty
            // space under it, dragged into to extend a selection. Land on the
            // document's last caret stop (its end), not offset 0: jumping the
            // caret to the top is the wrong direction, and 0 isn't even a stop
            // when the document opens on hidden frontmatter or a `# ` marker, so
            // returning it would leave the caret where it draws in one place and
            // types in another (`move_to` would then clamp it onto the unhomeable
            // frontmatter floor). `None` only for a document with no stops at all
            // (empty), where the caret has nowhere to be but 0.
            return self.stops.last().copied().unwrap_or(0);
        };
        match r.glyph_at_col(col).and_then(|i| r.glyphs.get(i)) {
            // A glyph that holds no caret is clickable, but where it points
            // isn't always somewhere the caret can be: the blank gap between two
            // paragraphs stands at an offset that belongs to neither of them,
            // and the tail of a grapheme cluster stands inside a character.
            // Land on the nearest real stop instead of handing back an offset
            // that looks like the gap but types into the paragraph above.
            Some(g) if !g.stop => self.nearest_stop(g.src),
            Some(g) => g.src,
            // A row's end is a stop by construction — unless the row is
            // decoration, which contributes none.
            None if r.decoration => self.nearest_stop(r.end_src),
            None => r.end_src,
        }
    }

    /// Which of a block media's two caret homes `off` is, or `None` for every
    /// other offset in the document.
    ///
    /// [`block_media`](Builder::block_media) gives a block-level image, video, or
    /// audio exactly two stops — one in front of it and one just past it — and
    /// nothing inside the markup. Both are ordinary offsets to everything else in
    /// core, but they are the two places where inserting text would *dissolve the
    /// picture*: `![](p.png)` with anything typed against it is no longer a block
    /// image but a paragraph with an inline one, and the frontend that was
    /// painting a photo there paints a text run instead. A caller that is about to
    /// insert asks this so it can open a paragraph first — see
    /// [`Doc::insert`](crate::Doc::insert).
    ///
    /// An *inline* image reports `None`: it has no placeholder row and no stops of
    /// its own, and typing beside one is ordinary editing.
    ///
    /// Answers with the media's own source span as well, since a caller that has
    /// to keep the picture whole usually has to address it — [`Doc::backspace`]
    /// takes the picture out in one piece rather than nibbling a byte off its
    /// markup, which is the same dissolution from the other side.
    ///
    /// [`Doc::backspace`]: crate::Doc::backspace
    pub fn block_media_stop(&self, off: usize) -> Option<(MediaStop, Range<usize>)> {
        self.media
            .iter()
            .find_map(|m| self.placeholder_stop(m.rows_span.start, off))
    }

    /// Which of a block leaf directive's two caret homes `off` is, or `None`
    /// for every other offset — [`block_media_stop`](Self::block_media_stop)
    /// for the [`directives`](Self::directives), for the same reason.
    ///
    /// A leaf directive is drawn on the picture's recipe (`⧉ label`, one stop
    /// in front of it and one just past it), and dissolves the same way: text
    /// typed against `::embed{src=x}` either end is a paragraph of raw source
    /// (`X::embed{src=x}`, `::embed{src=x}X`), and djot's empty `::: name`
    /// fence either stops opening or stops closing. `::page-break` is one of
    /// these. Answers with the directive's own span, as its peer does.
    pub fn block_directive_stop(&self, off: usize) -> Option<(MediaStop, Range<usize>)> {
        self.directives
            .iter()
            .find_map(|d| self.placeholder_stop(d.rows_span.start, off))
    }

    /// Whether `off` is one of the two stops of the placeholder row `row` —
    /// a block picture's or a leaf directive's, which share the recipe — and
    /// the span the placeholder stands for.
    fn placeholder_stop(&self, row: usize, off: usize) -> Option<(MediaStop, Range<usize>)> {
        let row = self.rows.get(row)?;
        // Every glyph of the `🖼 alt` / `⧉ label` label maps to the block's
        // start offset; the row's end is past its markup. Read the start off
        // the label rather than the first glyph, which on a quoted or listed
        // picture is the block prefix and points at the gutter.
        let start = row
            .glyphs
            .iter()
            .find(|g| g.style.role == Role::Image)
            .map(|g| g.src)?;
        if off == start {
            return Some((MediaStop::Before, start..row.end_src));
        }
        if off == row.end_src {
            return Some((MediaStop::After, start..row.end_src));
        }
        None
    }

    /// Whether `off` is a table's trailing caret stop — the one home past a
    /// table's last cell, at the block's own end ([`TableInfo::end_src`]).
    ///
    /// The table's peer of [`block_media_stop`](Self::block_media_stop)'s
    /// `After`: text inserted at that offset joins the table's last source
    /// line, and a line glued under a table is a row of it (`| 1 | 2 |x`), so
    /// a caller about to insert there opens a paragraph first — see
    /// [`Doc::insert`](crate::Doc::insert). Nothing else about the offset is
    /// special: it is where Down from the last row lands and where a click in
    /// the blank space under a trailing table lands.
    pub fn table_end_stop(&self, off: usize) -> bool {
        self.tables.iter().any(|t| t.end_src == off)
    }

    /// Snap `off` to the nearest caret stop — the funnel a frontend that
    /// hit-tests pixels straight to a source offset must run its result through.
    /// A click or drag can land in the blank gap a paragraph break is drawn with,
    /// or inside a hidden delimiter; both are offsets the caret can't rest at, so
    /// resting there would draw the caret in one place and type in another. This
    /// settles it on a real caret home instead. Idempotent on an offset that is
    /// already a stop — the `(row, col)` click path already snaps this way inside
    /// [`offset_of_pos`](Self::offset_of_pos), and this gives the pixel path the
    /// same guarantee. Returns `off` unchanged only for an empty document (no
    /// stops at all).
    pub fn snap_to_stop(&self, off: usize) -> usize {
        self.nearest_stop(off)
    }

    /// The caret stop nearest `off`, preferring the one before it when `off`
    /// falls exactly between two. Returns `off` unchanged if there are no stops
    /// at all (an empty document). A mark's content end counts: it is a place
    /// the caret rests, and the one a drag ending on a marked word means.
    fn nearest_stop(&self, off: usize) -> usize {
        let before = Self::last_at_or_before(&self.stops, off)
            .max(Self::last_at_or_before(&self.mark_ends, off));
        let after = match (
            Self::first_at_or_after(&self.stops, off),
            Self::first_at_or_after(&self.mark_ends, off),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match (before, after) {
            (Some(b), Some(a)) if off - b <= a - off => b,
            (_, Some(a)) => a,
            (Some(b), None) => b,
            (None, None) => off,
        }
    }

    /// The glyph stop nearest `off` — [`nearest_stop`](Self::nearest_stop)
    /// for a walk that pairs stops with characters, which a mark's content
    /// end has none of. A caret resting on one resolves to the glyph stop
    /// drawn at the same spot, the one just past the hidden delimiter, so the
    /// text a system input is shown from there and the steps it counts agree.
    pub fn snap_to_glyph_stop(&self, off: usize) -> usize {
        if self.mark_ends.binary_search(&off).is_ok()
            && let Some(next) = Self::first_at_or_after(&self.stops, off)
        {
            return next;
        }
        let before = Self::last_at_or_before(&self.stops, off);
        let after = Self::first_at_or_after(&self.stops, off);
        match (before, after) {
            (Some(b), Some(a)) if off - b <= a - off => b,
            (_, Some(a)) => a,
            (Some(b), None) => b,
            (None, None) => off,
        }
    }

    /// The last of `sorted` at or before `off`, if any.
    fn last_at_or_before(sorted: &[usize], off: usize) -> Option<usize> {
        let i = sorted.partition_point(|&s| s <= off);
        i.checked_sub(1).map(|i| sorted[i])
    }

    /// The first of `sorted` at or after `off`, if any.
    fn first_at_or_after(sorted: &[usize], off: usize) -> Option<usize> {
        let i = sorted.partition_point(|&s| s < off);
        sorted.get(i).copied()
    }

    /// The next place the caret rests past `off` — the next glyph stop or the
    /// next mark's content end, whichever comes first. What Right walks:
    /// leaving `**bold**` from the `d` is two presses, one onto the end of the
    /// bold (still bold, the toolbar lit) and one past its delimiter, at the
    /// same spot on screen. [`stop_after`](Self::stop_after) is the walk that
    /// skips the first, for every caller that pairs stops with characters.
    pub fn caret_stop_after(&self, off: usize) -> Option<usize> {
        match (
            self.stop_after(off),
            Self::first_at_or_after(&self.mark_ends, off + 1),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The previous place the caret rests before `off` — the mirror of
    /// [`caret_stop_after`](Self::caret_stop_after), what Left walks.
    pub fn caret_stop_before(&self, off: usize) -> Option<usize> {
        self.stop_before(off).max(
            off.checked_sub(1)
                .and_then(|o| Self::last_at_or_before(&self.mark_ends, o)),
        )
    }

    /// Whether the caret can occupy `row` at all: decoration rows (a table's
    /// border rules) are stepped over by vertical motion.
    pub fn row_is_navigable(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| !r.decoration)
    }

    /// The first offset the caret can rest at on `row` — its first stop, or the
    /// row's own end when it holds no text (an empty paragraph). `None` for a
    /// decoration row, which holds no caret at all.
    ///
    /// Not `offset_of_pos(row, 0)`: column 0 of a quoted or listed row is the
    /// gutter, and a gutter's `src` points at the *block* it opens, so the stop
    /// nearest it is the one on the block's first row rather than on this one.
    /// Which is right for a click — the gutter decorates the whole block — and
    /// wrong for Home, whose whole question is where *this* row starts.
    pub fn row_start(&self, row: usize) -> Option<usize> {
        let r = self.rows.get(row).filter(|r| !r.decoration)?;
        Some(
            r.glyphs
                .iter()
                .find(|g| g.stop)
                .map_or(r.end_src, |g| g.src),
        )
    }

    /// The last row the caret can rest on — the fallback when an offset is past
    /// everything rendered (a table's bottom border must not swallow the caret).
    pub(super) fn last_stop_row(&self) -> usize {
        (0..self.rows.len())
            .rev()
            .find(|&r| self.row_is_navigable(r))
            .unwrap_or(0)
    }

    /// The nearest row above `row` the caret can occupy, skipping decoration.
    pub fn navigable_above(&self, row: usize) -> Option<usize> {
        (0..row.min(self.rows.len()))
            .rev()
            .find(|&r| self.row_is_navigable(r))
    }

    /// The nearest row below `row` the caret can occupy, skipping decoration.
    pub fn navigable_below(&self, row: usize) -> Option<usize> {
        ((row + 1)..self.rows.len()).find(|&r| self.row_is_navigable(r))
    }

    /// The caret stop just before `off` — one press of Left. `None` at the
    /// first stop in the document.
    ///
    /// Runs of decoration (a table border, a cell's alignment padding) are
    /// stepped over in a single press: they hold no stop, so they aren't in the
    /// table to land on.
    pub fn stop_before(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s < off);
        i.checked_sub(1).map(|i| self.stops[i])
    }

    /// The caret stop just after `off` — one press of Right. `None` at the last
    /// stop in the document.
    pub fn stop_after(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s <= off);
        self.stops.get(i).copied()
    }

    /// The first caret stop at or past `off` — where the caret at a hidden
    /// offset is *drawn*, and so where a rightward walk over the rendered text
    /// starts from.
    pub fn stop_at_or_after(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s < off);
        self.stops.get(i).copied()
    }

    /// The last caret stop at or before `off` — where a leftward walk starts
    /// from. Snapping the way the walk is headed, rather than always forward,
    /// is what keeps a leftward motion from ever moving the caret right.
    pub fn stop_at_or_before(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s <= off);
        i.checked_sub(1).map(|i| self.stops[i])
    }

    /// Whether the caret may rest at `off` — the invariant every motion in this
    /// view has to leave standing. A glyph stop, a row's end, or a hidden
    /// mark's content end ([`mark_ends`](Self::mark_ends)).
    pub fn is_stop(&self, off: usize) -> bool {
        self.stops.binary_search(&off).is_ok() || self.mark_ends.binary_search(&off).is_ok()
    }

    /// The visible text a caret crosses walking rightward from `from` up to
    /// (but not including) `to` — `UITextInput.text(in:)`'s `[from, to)` in
    /// *this* view. A hidden inline-mark delimiter (`**`, `` ` ``, `_`, an
    /// escape backslash) never got a glyph in the first place — see
    /// [`push_text`]/[`synth`] — so it contributes nothing; what's left is
    /// what's drawn on screen for that span, one character per caret stop.
    ///
    /// **Exactly one character per stop** is the contract, and it is the
    /// system text input's, not a nicety: `UITextInput`'s tokenizer reads a
    /// window of this text around a tap, indexes into it by the integer
    /// `offset(from:to:)` reports (`distance_offset` in `leaf-ffi`, a count of
    /// [`stop_after`](Self::stop_after) hops), finds a word boundary at some
    /// character index, and hands the delta back through
    /// `position(from:offset:)`, which hops stops again. If the text ever
    /// spends a character on something that is not a stop, or a stop on
    /// nothing, every index past that point is off by one and the word the
    /// reader double-tapped comes back shifted — into the header row of a
    /// table, or one letter short. So a stop that draws a glyph is spelled
    /// as that glyph, and a stop that draws none is spelled `'\n'`:
    ///
    /// - a row's own end stop ([`VRow::end_src`]) — the caret home past a
    ///   paragraph's, heading's, list item's, or code line's last glyph. This
    ///   is also what keeps two blocks' words apart: without it the last word
    ///   of one paragraph and the first of the next read as one run of
    ///   letters (`"…edb\n\nhello\n"` came back as `"edbhello"`), and the
    ///   tokenizer selected across the boundary. A list item's end is a
    ///   row end like any other, though no blank gap row follows it.
    /// - a table cell's end, which [`push_table_row`] draws as the gutter
    ///   space before the next `│` so the caret has somewhere to stand past
    ///   the cell's last character. To a reader of *this* text a cell ends a
    ///   line: spelled as a space, a touch surface that lands a tap at a
    ///   word's end past the space that follows it stepped into the next
    ///   cell — or the next row, from the last column.
    ///
    /// A hidden mark's content end ([`mark_ends`](Self::mark_ends)) is a place
    /// the caret rests but not a stop the walks above count, so it has no
    /// character here either; `from` is snapped to the glyph stop drawn at
    /// the same spot first, exactly as [`snap_to_glyph_stop`] does for those
    /// walks. `to` is left as given, so a stop landing exactly on it is still
    /// excluded — the same half-open range `distance_offset`'s loop counts.
    ///
    /// [`push_table_row`]: Builder::push_table_row
    /// [`snap_to_glyph_stop`]: Self::snap_to_glyph_stop
    pub fn visible_text(&self, from: usize, to: usize) -> String {
        self.visible_items(from, to)
            .into_iter()
            .map(|(_, ch)| ch.unwrap_or('\n'))
            .collect()
    }

    /// The UTF-16 length of `visible_text(from, to)` — what an `NSRange`
    /// location into that text is, without building the string.
    ///
    /// AppKit's `NSTextInputClient` and `NSAccessibility` speak in UTF-16
    /// units of *the text as the system sees it*, which for leaf is the visible
    /// text — delimiters hidden. A frontend reporting its selection to the
    /// system converts each end with this and gets back an index into the
    /// string `visible_text(0, end)` returns, which is exactly what the system
    /// will index into.
    pub fn visible_utf16_len(&self, from: usize, to: usize) -> usize {
        let (lo, hi) = self.visible_span(from, to);
        let utf16 = &self.spelling().utf16;
        utf16[hi] - utf16[lo]
    }

    /// The inverse of `visible_utf16_len(0, ·)`: the source offset of the
    /// visible character a UTF-16 index into `visible_text(0, to)` lands on.
    ///
    /// An index inside a surrogate pair resolves to the character that owns
    /// it; one at or past the end of the text returns `None`, so a caller can
    /// substitute the document's end stop. The `\n` a row's or a cell's end
    /// is spelled with resolves to that end stop — a caret home, so a caller
    /// placing a caret there needs no snap.
    pub fn offset_at_visible_utf16(&self, to: usize, index: usize) -> Option<usize> {
        let (lo, hi) = self.visible_span(0, to);
        let utf16 = &self.spelling().utf16;
        // The stop whose text ends past the index is the one it lands on; a
        // stop whose text ends at or before it lies wholly before it.
        let target = utf16[lo] + index;
        let i = lo + utf16[lo + 1..=hi].partition_point(|&end| end <= target);
        (i < hi).then(|| self.stops[i])
    }

    /// The items `visible_text` spells, in order — one per caret stop in
    /// `[from, to)`, keyed by the stop's source offset: the glyph it draws
    /// (`Some`), or `None` for a stop with no character of its own, which the
    /// text spells `'\n'`. See [`visible_text`](Self::visible_text) for which
    /// stops those are and why.
    pub(super) fn visible_items(&self, from: usize, to: usize) -> Vec<(usize, Option<char>)> {
        let (lo, hi) = self.visible_span(from, to);
        self.stops[lo..hi]
            .iter()
            .copied()
            .zip(self.spelling().chars[lo..hi].iter().copied())
            .collect()
    }

    /// The stops `visible_text(from, to)` spells, as a range into the stop
    /// table: from the glyph stop `from` snaps to, up to but excluding the
    /// first stop at or past `to`.
    pub(super) fn visible_span(&self, from: usize, to: usize) -> (usize, usize) {
        let from = self.snap_to_glyph_stop(from);
        let lo = self.stops.partition_point(|&s| s < from);
        // The document's last stop is the end of the text, not a character in
        // it: `distance_offset` has no hop past it to pair one with.
        let last = self.stops.len().saturating_sub(1);
        let hi = self.stops.partition_point(|&s| s < to).min(last).max(lo);
        (lo, hi)
    }

    /// The [`Spelling`] of this map's stops, tabulated on first use.
    fn spelling(&self) -> &Spelling {
        self.spelling.get_or_init(|| {
            // The glyph each stop draws — the first at its offset in row
            // order, since a media row's label glyphs all share the media's
            // offset and a wrapped line's end is the next line's first glyph.
            // Row order only follows source order outside a table's wrapped
            // cells (see `pos_of_offset`), which is why each glyph looks its
            // stop up rather than the two being walked side by side.
            //
            // Within a row offsets ascend, so a cursor into the stop table
            // walks forward with the row's glyphs and the whole pass is a
            // read of the glyphs plus one search per row — a keystroke's
            // first conversion pays for this, so it is kept to that. The
            // search is repeated only if a row's offsets step back, which
            // none does; the walk is correct either way.
            let stops = &self.stops;
            let mut chars: Vec<Option<char>> = vec![None; stops.len()];
            for row in self.rows.iter().filter(|r| !r.decoration) {
                let mut i = 0;
                let mut prev = usize::MAX;
                for g in row.glyphs.iter().filter(|g| g.stop) {
                    if prev == usize::MAX || g.src < prev {
                        i = stops.partition_point(|&s| s < g.src);
                    } else {
                        while i < stops.len() && stops[i] < g.src {
                            i += 1;
                        }
                    }
                    prev = g.src;
                    if i < stops.len() && stops[i] == g.src && chars[i].is_none() {
                        chars[i] = Some(g.ch);
                    }
                }
            }
            // A cell's end stop has a glyph (the gutter space) but is spelled
            // as a line end; the structural grid is where the cells' offsets
            // live.
            for cell in self
                .tables
                .iter()
                .flat_map(|t| t.grid.iter())
                .flat_map(|r| r.cells.iter())
            {
                if let Ok(i) = self.stops.binary_search(&cell.end) {
                    chars[i] = None;
                }
            }
            let mut utf16 = Vec::with_capacity(chars.len() + 1);
            utf16.push(0);
            for ch in &chars {
                let before = *utf16.last().unwrap_or(&0);
                utf16.push(before + ch.map_or(1, char::len_utf16));
            }
            Spelling { chars, utf16 }
        })
    }
}
