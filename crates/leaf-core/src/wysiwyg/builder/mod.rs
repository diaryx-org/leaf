//! The AST → glyph walker: one `Builder` per build, emitting rows block by block.

use super::*;

mod blocks;
mod embeds;
mod inline;
mod rows;
mod table;
mod verse;

pub(super) struct Builder<'a> {
    pub(super) nodes: &'a [FlatNode],
    /// The document source, consulted to place blank-line rows at the source
    /// offsets the caret should occupy on them (the AST drops blank lines).
    pub(super) source: &'a str,
    /// The word-wrap column budget, or `None` to emit each block as a single
    /// unwrapped row (the frontend wraps).
    pub(super) wrap: Option<usize>,
    pub(super) rows: Vec<VRow>,
    /// Built alongside `rows`, never instead of them — see [`TableInfo`].
    pub(super) tables: Vec<TableInfo>,
    /// The end offset of the last content emitted — the anchor for blank
    /// separator rows so the caret never snaps onto one.
    pub(super) last_off: usize,
    /// The end of the last block the walk stepped over without drawing — a
    /// comment, which the rich view hides. `last_off` moves past it too, for the
    /// separators; this is kept apart so the trailing blank lines can be counted
    /// from it without also being counted from a code block's closing fence,
    /// which `last_off` likewise ends after. `0` until a hidden block is met.
    pub(super) stepped_over: usize,
    /// How many rows each block image reserves, keyed by its destination — the
    /// frontend's per-image height, threaded in from [`crate::Doc::set_media_rows`]
    /// so [`Builder::block_media`] can size the placeholder without core doing any
    /// I/O. A destination absent from the map (or a `0`/`1` entry) reserves the
    /// bare one-row placeholder, which is the whole-document default and what
    /// every existing test — passing an empty map — still gets.
    pub(super) surface: &'a Surface,
    /// The glyph a hard break renders as while the current inline run is built:
    /// a space in prose (a break folds into the flow the frontend wraps), but a
    /// newline (`\n`) inside a table cell, where a row is one source line and the
    /// only break it can carry is an explicit one that must show as a line of its
    /// own. Set around [`Builder::row_cells`] and otherwise left at `' '`.
    pub(super) break_glyph: Cell<char>,
    /// Render a soft break (a bare newline inside a paragraph) as a line break
    /// where it was written, rather than folding it into the reflowed paragraph
    /// — the `LineFlow::Preserve` behaviour. A soft break emits a `'\n'` glyph
    /// (like a hard break in a cell), which [`Builder::emit_wrapped`] turns into
    /// a fresh visual row. `false` is the flowing-prose default. Inside a table
    /// cell (where `break_glyph` is already `'\n'`) it has no effect: a cell is
    /// one line and folds its own soft breaks regardless.
    pub(super) preserve_soft: bool,
    /// The source byte range of the one line that should render its markup
    /// *raw* — the caret's line under `MarkupMode::Full` (see
    /// [`crate::Doc::reveal_line`]). `None` in every other mode and view, which
    /// is the delimiters-always-hidden behaviour every build had before the
    /// preference existed.
    ///
    /// Read only by [`Builder::revealed`], which every delimiter-bearing arm of
    /// [`Builder::inline`] consults. A range rather than a bare caret offset
    /// because the decision is per-*node*, not per-caret: a node is revealed
    /// when its span meets this line, so `*em*` shows both its asterisks even
    /// with the caret at one end of it.
    pub(super) reveal: Option<Reveal>,
    /// The content ends of the hidden marks rendered since the last row was
    /// pushed — recorded as the inline walk meets each mark, and drained onto
    /// the rows as they are emitted (see [`Builder::take_mark_ends`]). A cell
    /// rather than a `&mut`, for the reason `break_glyph` is: the inline walk
    /// borrows the builder shared.
    pub(super) pending_mark_ends: RefCell<Vec<usize>>,
    /// The inline atoms rendered since the last row was pushed, keyed by the
    /// formula's start offset — the offset the atom glyph carries, which is
    /// how [`Builder::take_math`] pairs each with its glyph once the wrap has
    /// decided which row it landed on. Drained the way `pending_mark_ends` is.
    pub(super) pending_math: RefCell<Vec<(usize, MathMark)>>,
    /// The owned inline regions rendered since the last row was pushed, keyed
    /// by start offset as `pending_math` is, and taken the same way
    /// ([`Builder::take_inline_regions`]).
    pub(super) pending_regions: RefCell<Vec<(usize, InlineRegionMark)>>,
    /// Whether this walk met a formula at all, atom, block, or revealed — the
    /// fact [`BlockCache`] keeps per block so [`crate::Doc::reveal_line`] can
    /// tell whether the caret's line has anything to reveal without walking.
    pub(super) saw_math: Cell<bool>,
    /// The presentation vocabulary in force at the block being walked — the
    /// keys the `div`s around it carry, folded together with the nearest
    /// winning, and [`Presentation::default`] at the top level.
    ///
    /// Saved and restored around each `div` in [`Builder::block`], so a block
    /// reads its own attributes over whatever its containers said and nothing
    /// leaks sideways to the block after it. It is per-*build* state rather
    /// than a parameter because every one of the dozen call sites of `block`
    /// would otherwise thread a value none of them care about.
    pub(super) presentation: Presentation,
    /// The named families this walk has met, by the id its glyphs carry — see
    /// [`VisualMap::faces`]. A `RefCell` for [`pending_mark_ends`]'s reason:
    /// the inline walk borrows the builder shared, and a span's `data-font` is
    /// read from inside it.
    ///
    /// [`pending_mark_ends`]: Builder::pending_mark_ends
    pub(super) faces: RefCell<FaceTable>,
}

/// The six presentation keys as the walker carries them down a block tree —
/// the two that are the block's ([`Align`], [`LineHeight`]) and the three that
/// are a run's but may be written on the block ([`FontSize`], [`FaceRef`],
/// [`TextColor`]).
///
/// `Copy` and five `Option`s, because folding is the whole of what it does:
/// [`under`](Presentation::under) reads a container's attributes over an
/// existing set and a key the container does not name keeps the value it had.
/// That is the "nearest wins" rule stated once, rather than at each of the
/// three levels a key can be written at. A name and a value fold alike: the
/// nearer node wins whichever of the two forms either of them wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Presentation {
    pub(super) align: Option<Align>,
    pub(super) line_height: Option<LineHeight>,
    pub(super) size: Option<FontSize>,
    pub(super) font: Option<FaceRef>,
    pub(super) color: Option<TextColor>,
}

impl Presentation {
    /// This set with whatever `attrs` names written over it — the nearer node's
    /// answer where it has one, the outer node's where it hasn't.
    ///
    /// `faces` is the build's intern table, which a named family is recorded in
    /// on the way past: the glyph carries the id and the table carries the
    /// string. Shared rather than `&mut` because the inline walk this feeds
    /// borrows the builder shared, the way `pending_mark_ends` does.
    fn under(self, attrs: &[(String, Option<String>)], faces: &RefCell<FaceTable>) -> Self {
        Self {
            align: Align::from_attrs(attrs).or(self.align),
            line_height: LineHeight::from_attrs(attrs).or(self.line_height),
            size: FontSize::from_attrs(attrs).or(self.size),
            font: faces.borrow_mut().face_from_attrs(attrs).or(self.font),
            color: TextColor::from_attrs(attrs).or(self.color),
        }
    }

    /// `base` carrying the three run-level keys — the style a block's glyphs
    /// start from, which an attributed span inside it then writes over.
    fn over(self, base: Style) -> Style {
        base.size(self.size).font(self.font).color(self.color)
    }
}

impl Builder<'_> {
    /// Note that the mark `id` closes with a hidden delimiter, so its content
    /// end is a caret home — unless the mark is empty, where the end is the
    /// start and there is nothing to extend.
    fn note_mark_end(&self, id: usize) {
        let node = &self.nodes[id];
        if let Some(content) = &node.content_span
            && content.end < node.span.end
            && !content.is_empty()
        {
            self.pending_mark_ends.borrow_mut().push(content.end);
        }
    }

    /// The pending mark ends at or before `end_src`, for the row ending there
    /// — every mark rendered so far that closes on it. A mark's end never
    /// exceeds the end of the row its last glyph is on, so the leftovers are
    /// those of rows still to come.
    fn take_mark_ends(&self, end_src: usize) -> Vec<usize> {
        let mut pending = self.pending_mark_ends.borrow_mut();
        let (taken, kept): (Vec<usize>, Vec<usize>) =
            pending.drain(..).partition(|&o| o <= end_src);
        *pending = kept;
        taken
    }

    /// The pending inline atoms whose glyph is on the row `glyphs` is about
    /// to become — each paired with the index of the [`Role::Math`] glyph
    /// carrying its offset, in glyph order. An atom whose glyph landed on an
    /// earlier row was taken then; one on a later row is left for it. The
    /// peer of [`take_mark_ends`](Self::take_mark_ends), and why an atom is
    /// keyed by offset: the wrap decides the row, and the offset is what the
    /// glyph still carries once it has.
    fn take_math(&self, glyphs: &[Glyph]) -> Vec<MathMark> {
        let mut pending = self.pending_math.borrow_mut();
        if pending.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, g) in glyphs.iter().enumerate() {
            if g.style.role != Role::Math || !g.stop {
                continue;
            }
            if let Some(at) = pending.iter().position(|(src, _)| *src == g.src) {
                let (_, mut mark) = pending.remove(at);
                mark.glyph = Some(i);
                out.push(mark);
            }
        }
        out
    }
    /// The pending inline regions whose atom is on the row `glyphs` is about
    /// to become, each with its atom's index — [`take_math`](Self::take_math)'s
    /// peer, for [`REGION_ATOM`] glyphs.
    fn take_inline_regions(&self, glyphs: &[Glyph]) -> Vec<InlineRegionMark> {
        let mut pending = self.pending_regions.borrow_mut();
        if pending.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, g) in glyphs.iter().enumerate() {
            if g.ch != REGION_ATOM || !g.stop {
                continue;
            }
            if let Some(at) = pending.iter().position(|(src, _)| *src == g.src) {
                let (_, mut mark) = pending.remove(at);
                mark.glyph = i;
                out.push(mark);
            }
        }
        out
    }

    /// Whether `span` belongs to the line that is showing its raw markup. True
    /// only when a reveal line is set *for markup* (`MarkupMode::Full`) and the
    /// two ranges actually meet — see [`Reveal::meets`] for what meeting is.
    fn revealed(&self, span: &Range<usize>) -> bool {
        self.reveal
            .as_ref()
            .is_some_and(|r| r.markup && r.meets(span))
    }

    /// Whether a formula at `span` shows its TeX rather than its picture: it
    /// meets the reveal line, in *any* mode. A formula's content is its source
    /// and not its picture, so for it the choice is not between a clean
    /// surface and a raw one but between editable and not — which is why this
    /// does not read [`Reveal::markup`] the way [`revealed`](Self::revealed)
    /// does.
    fn math_revealed(&self, span: &Range<usize>) -> bool {
        self.reveal.as_ref().is_some_and(|r| r.meets(span))
    }

    /// The `(opening, closing)` source byte ranges of a node's delimiters — the
    /// bytes its `span` holds that its `content_span` doesn't.
    ///
    /// This is how *every* inline delimiter is recovered, rather than a table of
    /// spellings per kind: twig gives `*em*` a span of `13..17` and a content
    /// span of `14..16`, so the gaps at each end are the delimiters, whatever
    /// they happen to be. That matters because one kind has many spellings —
    /// `*em*` and `_em_` are both emphasis, `` `x` `` and ``` ``x`` ``` both
    /// verbatim — and re-deriving the text from the source is the only way to
    /// show back what the author actually typed. It also gets a link's
    /// asymmetric `[` / `](dest)` right for free.
    ///
    /// `None` when the node has no content span, or when content and span
    /// coincide (nothing was elided, so there is nothing to reveal).
    fn delims(&self, id: usize) -> Option<(Range<usize>, Range<usize>)> {
        let node = &self.nodes[id];
        let content = node.content_span.clone()?;
        let span = node.span.clone();
        // A content span that escapes its own node's span means the two are
        // describing different things; reveal nothing rather than slice wildly.
        if content.start < span.start || content.end > span.end {
            return None;
        }
        let (open, close) = (span.start..content.start, content.end..span.end);
        // A delimiter that spans a newline isn't this line's to reveal — a setext
        // heading's `\n=====` underline is the case that arises in practice. It
        // would also inject a `'\n'` glyph, which `emit_wrapped` reads as a hard
        // row break, so the row would split where the author wrote no break.
        let multiline =
            |r: &Range<usize>| self.source.get(r.clone()).is_some_and(|s| s.contains('\n'));
        if multiline(&open) || multiline(&close) {
            return None;
        }
        (!open.is_empty() || !close.is_empty()).then_some((open, close))
    }

    /// Emit the source bytes of `range` as revealed markup — real glyphs, each
    /// mapped to its own source byte and each a caret stop, so a delimiter shown
    /// is a delimiter that can be selected, edited and deleted like any other
    /// text. Styled [`Role::Delimiter`] on top of the run's own style, which is
    /// how a frontend tells scaffolding from prose and dims it.
    ///
    /// Deliberately *not* [`push_escaped_text`]: this is raw source, not parsed
    /// text, so there is no escape-driven drift between the two to correct.
    fn push_delim(&self, out: &mut Vec<Glyph>, range: &Range<usize>, base: Style) {
        let Some(text) = self.source.get(range.clone()) else {
            return;
        };
        push_text(out, text, range.start, base.role(Role::Delimiter));
    }

    /// Render an inline node's children wrapped in its raw delimiters when the
    /// node is on the revealed line, and bare (delimiters resolved away) when it
    /// isn't — the shared body of every delimiter-bearing arm of
    /// [`inline`](Self::inline).
    ///
    /// `style` is the resolved styling the content still gets in *both* modes:
    /// revealing `*em*` shows the asterisks *and* keeps the text italic, the
    /// live-preview behaviour. Showing the markup is not the same as turning the
    /// rendering off — that is what [`crate::View::Source`] is for.
    fn inline_delimited(&self, id: usize, style: Style, out: &mut Vec<Glyph>) {
        let show = self
            .revealed(&self.nodes[id].span)
            .then(|| self.delims(id))
            .flatten();
        if let Some((open, _)) = &show {
            self.push_delim(out, open, style);
        }
        self.recurse(id, style, out);
        match &show {
            Some((_, close)) => self.push_delim(out, close, style),
            // Hidden, so the content's end has no glyph after it: give the
            // caret its home there.
            None => self.note_mark_end(id),
        }
    }

    /// A verbatim span — or a formula drawn as its TeX — in the code style:
    /// its text at its content's offset, bracketed by its raw delimiters when
    /// `show` says the line is revealed and by nothing (plus the caret's home
    /// at the content's end) when it isn't.
    ///
    /// Not [`inline_delimited`](Self::inline_delimited): verbatim has no child
    /// nodes to recurse into — its content is its own `text` — so the fences
    /// bracket a [`push_text`] instead. The fences themselves keep
    /// `Role::Code`'s sibling treatment via [`push_delim`]'s role override.
    ///
    /// [`push_delim`]: Self::push_delim
    fn inline_verbatim(&self, id: usize, base: Style, out: &mut Vec<Glyph>, show: bool) {
        let node = &self.nodes[id];
        // The interior begins at `content_span.start` — past however many
        // backticks the fence used, which `span.start + 1` only guessed right
        // for a single one. Fall back to that guess if it's absent.
        let at = node
            .content_span
            .as_ref()
            .map_or(node.span.start + 1, |c| c.start);
        let style = base.role(Role::Code);
        let show = show.then(|| self.delims(id)).flatten();
        if let Some((open, _)) = &show {
            self.push_delim(out, open, style);
        }
        push_text(out, node.text.as_deref().unwrap_or(""), at, style);
        match &show {
            Some((_, close)) => self.push_delim(out, close, style),
            None => self.note_mark_end(id),
        }
    }

    fn children(&self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut c = self.nodes[id].first_child;
        while let Some(cid) = c {
            out.push(cid.0 as usize);
            c = self.nodes[cid.0 as usize].next_sibling;
        }
        out
    }

    /// How many lists the list `id` sits inside: 0 for a top-level list, 1 for
    /// one nested in an item, and so on. Counted over list *items*, not lists,
    /// so a block twig hangs straight off a list (see the list arm's de-nested
    /// child) doesn't make a list inside it look nested. A block quote between
    /// two lists neither resets nor adds to the count: `- a` / `  > - b` draws
    /// `b` as the second level it is in the outline.
    fn list_depth(&self, id: usize) -> usize {
        let mut depth = 0;
        let mut p = self.nodes[id].parent;
        while let Some(pid) = p {
            let n = &self.nodes[pid.0 as usize];
            if matches!(n.kind, Kind::ListItem | Kind::TaskListItem) {
                depth += 1;
            }
            p = n.parent;
        }
        depth
    }

    /// The number an ordered list's first item spells (`5.` → 5), 1 when its
    /// marker has none to read — an HTML `<li>` has no marker text at all.
    fn list_start(&self, kids: &[usize]) -> usize {
        kids.iter()
            .find_map(|&k| self.nodes[k].marker_span.clone())
            .and_then(|m| self.source.get(m))
            .map(|t| t.trim_start_matches(|c: char| !c.is_ascii_digit()))
            .map(|t| &t[..t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len())])
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(1)
    }

    /// Render a node's block children, a blank separator between each. `tight`
    /// suppresses the *fabricated* separator between adjacent children that share
    /// a source line boundary — a tight list item and the sub-list nested in it —
    /// while a real blank source line between them still opens a gap.
    fn blocks(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph], tight: bool) {
        // Frontmatter (a leading `metadata` block) is document metadata, not
        // prose: hide it entirely in the rich-text view. Skipping it here means
        // no phantom blank rows for its lines and no separator before the first
        // real block — the document opens straight into its content.
        let kids: Vec<usize> = self
            .children(id)
            .into_iter()
            .filter(|&c| self.nodes[c].kind != Kind::Metadata)
            .collect();
        let mut above: Option<BlockClass> = None;
        for child in kids {
            let below = BlockClass::from_node_kind(&self.nodes[child].kind);
            let before_sep = self.rows.len();
            if let Some(above) = above {
                self.emit_separators_before(
                    self.nodes[child].span.start,
                    pc,
                    !tight,
                    Boundary { above, below },
                );
            }
            // The first *drawn* child wears the first-row prefix (a bullet, a
            // footnote label), not the first child: a comment opening a list
            // item draws nothing, and the bullet belongs to what follows it.
            let first = if above.is_none() { pf } else { pc };
            if self.block_or_hidden(child, before_sep, first, pc) {
                above = Some(below);
            }
        }
    }

    /// Render `child` after the separator [`Builder::emit_separators_before`]
    /// spelled for it from row `before_sep` on, and say whether it drew
    /// anything.
    ///
    /// A block that draws no rows — an HTML comment, which the rich view hides
    /// the way it hides frontmatter — is still *there* in the source, and the
    /// walk has to step over it: `last_off` moves past it so the next separator
    /// counts the blank lines from its end, not from wherever the last drawn
    /// block stopped. Left where it was, the separator counted every line of the
    /// comment as a blank row; and the cached path, whose per-block builder
    /// starts at offset 0, handed back a `last_off` of 0 and counted every line
    /// of the *document* — one phantom blank row per source line, once per
    /// comment. The separator drawn for it is taken back too, so a hidden block
    /// leaves no gap of its own: what stands either side of it meets across one
    /// boundary, as if the comment were not there.
    fn block_or_hidden(
        &mut self,
        child: usize,
        before_sep: usize,
        pf: &[Glyph],
        pc: &[Glyph],
    ) -> bool {
        let after_sep = self.rows.len();
        self.block(child, pf, pc);
        if self.rows.len() > after_sep {
            return true;
        }
        self.rows.truncate(before_sep);
        let end = self.nodes[child].span.end;
        self.last_off = self.last_off.max(end);
        self.stepped_over = self.stepped_over.max(end);
        false
    }
}
