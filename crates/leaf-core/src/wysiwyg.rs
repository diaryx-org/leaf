//! The WYSIWYG view: render the document with its markup *resolved*, not shown —
//! headings coloured, `**bold**` as real bold, `# ` / `**` / `` ` `` delimiters
//! hidden — while keeping every visible glyph tied back to the source byte it
//! came from.
//!
//! That back-reference (`Glyph::src`) is what lets a caret still work: the caret
//! stays a source offset (shared with the source view), but the [`VisualMap`]
//! converts between an offset and a screen `(row, col)`, so cursor drawing,
//! mouse clicks, and vertical motion all operate in *visible* space.
//!
//! Left and Right instead walk the map's caret *stops* in document order. On
//! ordinary prose that's the same journey — the stops are laid out left to right
//! — and it steps over the hidden delimiters either way. They part company only
//! in a table, where the text is arranged in two dimensions and a cell wrapped
//! within its column continues *below* rather than to the right. Following the
//! document is what a caret means there.
//!
//! Text is walked from the AST (`str` nodes carry exact spans, and their text is
//! the verbatim source slice), so a Markdown and a Djot file that parse alike
//! render — and map — identically.

use std::ops::Range;

use twig::{Alignment, FlatNode};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::style::{Color, Style};

/// One rendered character plus the source byte offset it originates from.
/// Synthetic glyphs (a list bullet, a quote gutter) point at their block's
/// start, so clicking one lands the caret at the start of that block.
#[derive(Clone)]
pub struct Glyph {
    pub ch: char,
    pub style: Style,
    pub src: usize,
    /// Whether the caret may *rest* on this glyph. Decoration — a table border
    /// or a cell's alignment padding — is visible but isn't text, so the caret
    /// steps over it instead of into it. It also can't be a stop even in
    /// principle: a run of decoration shares one `src`, and a caret can only
    /// move by changing offset, so resting on it would pin horizontal motion.
    /// A click still maps through `src`, which is why decoration points at the
    /// text it decorates.
    ///
    /// Real text is a stop once per *grapheme cluster*, on the glyph that opens
    /// it: the continuation glyphs of an emoji or an accented letter are drawn,
    /// but standing between them is standing inside a character.
    pub stop: bool,
}

/// One visual line. `end_src` is the source offset a caret sits at when placed
/// at the line's end (past its last glyph) — the anchor for end-of-line and
/// click-past-content.
pub struct VRow {
    pub glyphs: Vec<Glyph>,
    pub end_src: usize,
    /// A row that is drawn but holds no caret: a table's `├───┼───┤` rules, and
    /// the blank gap a block boundary is spelled with. Vertical motion steps
    /// over it, `pos_of_offset` never resolves onto it, and its stops (it has
    /// none) and `end_src` stay out of the map's stop table.
    ///
    /// Emptiness isn't the test — an empty paragraph is a blank row too, and a
    /// real caret stop. The test is whether the row is somewhere text can go.
    pub decoration: bool,
}

/// The rendered document plus the offset⇄position mapping the caret rides on.
#[derive(Default)]
pub struct VisualMap {
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
    stops: Vec<usize>,
    /// Every table in the document, in order, described structurally rather than
    /// drawn — see [`TableInfo`] for why both exist.
    pub tables: Vec<TableInfo>,
}

impl VisualMap {
    pub fn num_rows(&self) -> usize {
        self.rows.len()
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
    pub fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        let mut best: Option<(usize, usize, usize)> = None; // (src, row, col)
        for (r, row) in self.rows.iter().enumerate() {
            if row.decoration {
                continue;
            }
            // Offsets ascend *within* a row, so its first stop at or past `off`
            // is the best this row has to offer.
            let cand = row
                .glyphs
                .iter()
                .enumerate()
                .find(|(_, g)| g.stop && g.src >= off)
                .map(|(i, g)| (g.src, r, row.col_of_glyph(i)))
                .or_else(|| (row.end_src >= off).then_some((row.end_src, r, row.width())));
            if let Some(c) = cand {
                // `<=`, so a tie goes to the later row: the only offset two rows
                // both hold is a wrap boundary, and it belongs to the row below.
                if best.is_none_or(|b| c.0 <= b.0) {
                    best = Some(c);
                }
            }
            // A row's *first* stop never decreases from one row to the next —
            // true even across a table's wrapped cells, since a cell's lines run
            // downward. So once a row opens past the best found so far, no later
            // row can beat it and the scan stays proportional to `off`.
            if let (Some(b), Some(first)) = (best, row.glyphs.iter().find(|g| g.stop)) {
                if first.src > b.0 {
                    break;
                }
            }
        }
        match best {
            Some((_, r, c)) => (r, c),
            None => {
                let r = self.last_stop_row();
                (r, self.row_width(r))
            }
        }
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
            return 0;
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

    /// The caret stop nearest `off`, preferring the one before it when `off`
    /// falls exactly between two. Returns `off` unchanged if there are no stops
    /// at all (an empty document).
    fn nearest_stop(&self, off: usize) -> usize {
        let i = self.stops.partition_point(|&s| s < off);
        let after = self.stops.get(i).copied();
        let before = i.checked_sub(1).map(|j| self.stops[j]);
        match (before, after) {
            (Some(b), Some(a)) if off - b <= a - off => b,
            (_, Some(a)) => a,
            (Some(b), None) => b,
            (None, None) => off,
        }
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
        Some(r.glyphs.iter().find(|g| g.stop).map_or(r.end_src, |g| g.src))
    }

    /// The last row the caret can rest on — the fallback when an offset is past
    /// everything rendered (a table's bottom border must not swallow the caret).
    fn last_stop_row(&self) -> usize {
        (0..self.rows.len())
            .rev()
            .find(|&r| self.row_is_navigable(r))
            .unwrap_or(0)
    }

    /// The nearest row above `row` the caret can occupy, skipping decoration.
    pub fn navigable_above(&self, row: usize) -> Option<usize> {
        (0..row.min(self.rows.len())).rev().find(|&r| self.row_is_navigable(r))
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
    /// view has to leave standing.
    pub fn is_stop(&self, off: usize) -> bool {
        self.stops.binary_search(&off).is_ok()
    }
}

/// Collect the caret stops of a laid-out grid: every stop glyph's offset plus
/// every row's end, ascending and deduplicated. Duplicates are the norm rather
/// than the exception — a wrapped line's end is the same offset as the next
/// line's first glyph — and collapsing them is what makes one press of Left or
/// Right cross exactly one stop.
fn collect_stops(rows: &[VRow]) -> Vec<usize> {
    let mut stops: Vec<usize> = rows
        .iter()
        .filter(|r| !r.decoration)
        .flat_map(|r| {
            r.glyphs
                .iter()
                .filter(|g| g.stop)
                .map(|g| g.src)
                .chain(std::iter::once(r.end_src))
        })
        .collect();
    stops.sort_unstable();
    stops.dedup();
    stops
}

/// A horizontal rule's dash count when the map isn't wrapping to a column grid
/// (the GUI, which wraps at pixel width): a fixed, sane width the frontend can
/// paint or re-wrap, instead of a runaway count from an unbounded wrap width.
const UNWRAPPED_RULE_WIDTH: usize = 40;

/// Render the document to a [`VisualMap`]. `wrap` is the column budget for
/// word-wrapping (`Some` for the monospace TUI), or `None` to emit one row per
/// block — the GUI does its own proportional pixel wrapping over these rows.
/// Text and offsets come from the AST (`str` nodes carry the verbatim source
/// slice and an exact span), so the original source string isn't needed here.
pub fn build(nodes: &[FlatNode], source: &str, wrap: Option<usize>) -> VisualMap {
    let Some(doc) = nodes.iter().position(|n| n.kind == "doc") else {
        return VisualMap::default();
    };
    let mut b = Builder {
        nodes,
        source,
        wrap: wrap.map(|w| w.max(8)),
        rows: Vec::new(),
        tables: Vec::new(),
        last_off: 0,
    };
    b.blocks(doc, &[], &[]);
    b.emit_trailing_blank_lines();
    let content_start = first_content_offset(nodes, doc);
    let stops = collect_stops(&b.rows);
    VisualMap {
        rows: b.rows,
        content_start,
        stops,
        tables: b.tables,
    }
}

/// The source offset of the first *rendered* top-level block — the first child
/// of `doc` that isn't hidden frontmatter (a `metadata` node). Zero when the
/// document opens straight into content (or is nothing but frontmatter).
fn first_content_offset(nodes: &[FlatNode], doc: usize) -> usize {
    let mut child = nodes[doc].first_child;
    while let Some(cid) = child {
        let n = &nodes[cid.0 as usize];
        if n.kind != "metadata" {
            return n.span.start;
        }
        child = n.next_sibling;
    }
    0
}

struct Builder<'a> {
    nodes: &'a [FlatNode],
    /// The document source, consulted to place blank-line rows at the source
    /// offsets the caret should occupy on them (the AST drops blank lines).
    source: &'a str,
    /// The word-wrap column budget, or `None` to emit each block as a single
    /// unwrapped row (the frontend wraps).
    wrap: Option<usize>,
    rows: Vec<VRow>,
    /// Built alongside `rows`, never instead of them — see [`TableInfo`].
    tables: Vec<TableInfo>,
    /// The end offset of the last content emitted — the anchor for blank
    /// separator rows so the caret never snaps onto one.
    last_off: usize,
}

impl Builder<'_> {
    fn children(&self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut c = self.nodes[id].first_child;
        while let Some(cid) = c {
            out.push(cid.0 as usize);
            c = self.nodes[cid.0 as usize].next_sibling;
        }
        out
    }

    /// Render a node's block children, a blank separator between each.
    fn blocks(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        // Frontmatter (a leading `metadata` block) is document metadata, not
        // prose: hide it entirely in the rich-text view. Skipping it here means
        // no phantom blank rows for its lines and no separator before the first
        // real block — the document opens straight into its content.
        let kids: Vec<usize> = self
            .children(id)
            .into_iter()
            .filter(|&c| self.nodes[c].kind != "metadata")
            .collect();
        for (i, child) in kids.into_iter().enumerate() {
            if i > 0 {
                // The blank line(s) between two blocks are real caret stops, each
                // needing its *own* source offset — one strictly past the previous
                // block's content, else it collides with that block's last row
                // and `pos_of_offset` (first-match-wins) would resolve the caret
                // onto the wrong row, pinning downward motion there.
                //
                // One row *per* blank source line, not a single collapsed
                // separator: an empty paragraph opened between two blocks (Enter
                // in the gap, `…\n\n\n\n…`) must be a navigable empty row, not
                // vanish — else the caret in it snaps onto the *next* block's
                // start and Enter looks like it did nothing.
                let next_start = self.nodes[child].span.start;
                let mut offs = self.blank_rows_between(self.last_off, next_start);
                if offs.is_empty() {
                    // A tight gap with no blank line (e.g. a heading directly
                    // above its text): keep the one conventional separator row so
                    // blocks still breathe, as they always have.
                    offs.push(self.blank_line_offset(self.last_off, next_start));
                }
                let last = offs.len() - 1;
                for (k, end_src) in offs.into_iter().enumerate() {
                    // The blank line a boundary is *drawn* with isn't a place
                    // text can go. The first one closes the block above and the
                    // last one opens the block below — with a single blank line,
                    // the usual case, doing both at once. Typing on either just
                    // continues the paragraph it abuts, since the blank line it
                    // would need to be a paragraph of its own is the very line
                    // being typed on. So they're a gap, like a table's border:
                    // drawn, clickable, never a caret's home.
                    //
                    // The lines *between* them are the real ones. That's what
                    // Enter opens: it inserts a paragraph break (`\n\n`), which
                    // leaves a blank line spare on each side and the caret on the
                    // navigable line between them.
                    self.rows.push(VRow {
                        glyphs: pc.to_vec(),
                        end_src,
                        decoration: k == 0 || k == last,
                    });
                }
            }
            let first = if i == 0 { pf } else { pc };
            self.block(child, first, pc);
        }
    }

    fn block(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "doc" | "section" => self.blocks(id, pf, pc),
            "heading" => {
                let style = heading_style(node.level.unwrap_or(1));
                let glyphs = self.inline_children(id, style);
                self.emit_wrapped(glyphs, node.span.start, pf, pc);
            }
            "block_quote" => {
                let gutter = synth("│ ", Color::Green, node.span.start);
                let f = concat(pf, &gutter);
                let c = concat(pc, &gutter);
                self.blocks(id, &f, &c);
            }
            "bullet_list" | "ordered_list" | "task_list" => {
                let ordered = node.kind == "ordered_list";
                for (i, item) in self.children(id).into_iter().enumerate() {
                    let start = self.nodes[item].span.start;
                    let marker = if ordered {
                        format!("{}. ", i + 1)
                    } else {
                        "• ".to_string()
                    };
                    let bullet = synth(&marker, Color::Yellow, start);
                    let indent = synth(&" ".repeat(text_width(&marker)), Color::Default, start);
                    self.block(item, &concat(pc, &bullet), &concat(pc, &indent));
                }
            }
            "list_item" | "task_list_item" => self.blocks(id, pf, pc),
            "table" => self.table(id, pf, pc),
            "code_block" => {
                let style = Style::default().fg(Color::Green);
                let text = node.text.clone().unwrap_or_default();
                let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
                // Each line at its own source offset, so the caret can walk the
                // code a character at a time like any other text. Where the
                // lines can't be lined up with the source there's no honest
                // offset to give, so the block maps coarsely to its start (and
                // stays a source-view job, as all of it once was).
                let offs = node
                    .content_span
                    .as_ref()
                    .and_then(|c| self.code_line_offsets(c, &lines));
                for (i, raw) in lines.iter().enumerate() {
                    let at = offs.as_ref().map_or(node.span.start, |o| o[i]);
                    let gutter = synth("▏ ", Color::DarkGray, at);
                    let mut glyphs: Vec<Glyph> = concat(pf, &gutter);
                    push_text(&mut glyphs, raw, at, style);
                    // Explicitly past the line's *text*: a blank line has only
                    // the gutter, whose offset would put the row's end inside
                    // the next line.
                    self.push_row_at(glyphs, at + raw.len());
                }
            }
            "thematic_break" => {
                let full = self.wrap.unwrap_or(UNWRAPPED_RULE_WIDTH);
                let w = full.saturating_sub(prefix_width(pf)).max(4);
                let mut glyphs = pf.to_vec();
                for _ in 0..w {
                    glyphs.push(Glyph {
                        ch: '─',
                        style: Style::default().fg(Color::DarkGray),
                        src: node.span.start,
                        // A rule is a block the caret can sit on, as it always
                        // has; it maps coarsely to the block's start.
                        stop: true,
                    });
                }
                self.push_row(glyphs, node.span.start);
            }
            _ => {
                // A container of blocks, or an inline-bearing paragraph.
                let kids = self.children(id);
                let inline = !kids.is_empty() && kids.iter().all(|&c| is_inline(&self.nodes[c].kind));
                if inline || kids.is_empty() {
                    let glyphs = self.inline_children(id, Style::default());
                    if !glyphs.is_empty() {
                        self.emit_wrapped(glyphs, node.span.start, pf, pc);
                    }
                } else {
                    self.blocks(id, pf, pc);
                }
            }
        }
    }

    /// Render a table as a box-drawn grid: every column as wide as its widest
    /// cell, the header bold and ruled off, each cell padded to its column's
    /// alignment.
    ///
    /// The alignment comes from twig's `cell.alignment` — the delimiter row
    /// (`|:--|--:|`) that spells it out is consumed by the parser and leaves no
    /// node, so the snapshot is the only source for it.
    ///
    /// Borders and padding are *decoration*: they carry the source offset of the
    /// text they surround, so a click lands in that cell, but they're never
    /// caret stops — the caret steps cell-to-cell instead of into the box art.
    fn table(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node_end = self.nodes[id].span.end;
        // twig's shape is `[caption, row, row, …]`: the caption is always
        // present (usually empty in Markdown) and is not part of the grid.
        let row_ids: Vec<usize> = self
            .children(id)
            .into_iter()
            .filter(|&c| self.nodes[c].kind == "row")
            .collect();
        if row_ids.is_empty() {
            return;
        }
        // Lay every cell out first — the column widths depend on all of them.
        let grid: Vec<Vec<TableCell>> = row_ids.iter().map(|&r| self.row_cells(r)).collect();
        let heads: Vec<bool> = row_ids
            .iter()
            .map(|&r| self.nodes[r].head.unwrap_or(false))
            .collect();
        let cols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![0usize; cols];
        for row in &grid {
            for (c, cell) in row.iter().enumerate() {
                widths[c] = widths[c].max(glyphs_width(&cell.glyphs));
            }
        }
        // Every column at its widest cell is only the *wish*; a grid wider than
        // the surface has its far side hanging off the edge where no amount of
        // caret motion can reach it. Cut it down to what's actually there, and
        // let the cells wrap into the space they're given.
        if let Some(w) = self.wrap {
            fit_widths(&mut widths, w.saturating_sub(prefix_width(pc)));
        }

        // Where the picture starts, so a frontend drawing its own grid knows
        // which rows to skip. Recorded before the first border goes down.
        let rows_start = self.rows.len();

        let anchor = grid[0].first().map(|c| c.start).unwrap_or(node_end);
        self.push_rule(&rule_text(&widths, '┌', '┬', '┐'), anchor, pf);
        for (ri, row) in grid.iter().enumerate() {
            self.push_table_row(row, &widths, pc);
            // The rule under the header: only where the head actually ends.
            let ends_head = heads[ri] && heads.get(ri + 1) == Some(&false);
            if ends_head {
                let next = grid[ri + 1].first().map(|c| c.start).unwrap_or(node_end);
                self.push_rule(&rule_text(&widths, '├', '┼', '┤'), next, pc);
            }
        }
        self.push_rule(&rule_text(&widths, '└', '┴', '┘'), node_end, pc);

        // The same cells the picture above was drawn from, published unwrapped
        // and unpadded for a frontend that lays them out in pixels.
        self.tables.push(TableInfo {
            rows_span: rows_start..self.rows.len(),
            end_src: node_end,
            // The *continuation* prefix: `pf` opens the block and only its first
            // row wears it, but every row of a grid is a continuation of the
            // block the table sits in.
            prefix: pc.to_vec(),
            grid: grid
                .into_iter()
                .zip(heads)
                .map(|(cells, head)| TableRow { head, cells })
                .collect(),
        });
        // The table's own end anchors whatever separator follows it; the border
        // rows deliberately don't move `last_off` (they hold no content).
        self.last_off = node_end;
    }

    /// One row of laid-out cells, in column order.
    fn row_cells(&self, row: usize) -> Vec<TableCell> {
        self.children(row)
            .into_iter()
            .filter(|&c| self.nodes[c].kind == "cell")
            .map(|c| {
                let n = &self.nodes[c];
                let style = if n.head.unwrap_or(false) {
                    Style::default().bold()
                } else {
                    Style::default()
                };
                // A cell's own `span` is the whole row; only `content_span`
                // bounds its text.
                let span = n.content_span.clone().unwrap_or(n.span.start..n.span.start);
                TableCell {
                    glyphs: self.inline_children(c, style),
                    start: span.start,
                    end: span.end,
                    align: n.alignment.unwrap_or(Alignment::Default),
                }
            })
            .collect()
    }

    /// A horizontal rule between/around rows — entirely decoration.
    fn push_rule(&mut self, text: &str, src: usize, prefix: &[Glyph]) {
        let glyphs = concat(prefix, &synth(text, Color::DarkGray, src));
        self.rows.push(VRow {
            glyphs,
            end_src: src,
            decoration: true,
        });
    }

    /// One `│ a │ b │` row of the grid: real cell text between decoration.
    ///
    /// A row of cells is not a row of the screen — a cell wrapped to its column
    /// spans several, each one `│`-divided across the full width so the grid
    /// stays square. Cells in the same row are laid out independently and run
    /// out at their own heights; a column that has run dry pads out as
    /// decoration while its neighbours keep going.
    fn push_table_row(&mut self, cells: &[TableCell], widths: &[usize], prefix: &[Glyph]) {
        let fallback = cells.last().map(|c| c.end).unwrap_or(0);
        let laid: Vec<Vec<Vec<Glyph>>> = cells
            .iter()
            .enumerate()
            .map(|(ci, c)| wrap_glyphs(&c.glyphs, widths.get(ci).copied().unwrap_or(0)))
            .collect();
        let height = laid.iter().map(|l| l.len()).max().unwrap_or(1).max(1);

        for j in 0..height {
            let mut glyphs = prefix.to_vec();
            for (ci, &w) in widths.iter().enumerate() {
                let cell = cells.get(ci);
                let line = laid.get(ci).and_then(|l| l.get(j));
                // The divider before this column belongs to the cell it
                // introduces, so clicking it lands in that cell — on this line
                // of it, which is what's next to the divider being clicked.
                let at = line
                    .and_then(|l| l.first().map(|g| g.src))
                    .or_else(|| cell.map(|c| c.start))
                    .unwrap_or(fallback);
                glyphs.extend(synth("│", Color::DarkGray, at));
                match (cell, line) {
                    (Some(cell), Some(line)) => {
                        let pad = w.saturating_sub(glyphs_width(line));
                        let (lead, trail) = match cell.align {
                            Alignment::Right => (pad, 0),
                            Alignment::Center => (pad / 2, pad - pad / 2),
                            Alignment::Left | Alignment::Default => (0, pad),
                        };
                        // Every line renders at least one space after its text
                        // (the gutter before `│`), so there is always somewhere
                        // to put the "after the last character" caret a line
                        // needs. It's the one padding glyph that is a stop: on
                        // the cell's last line that's the cell's end, and on any
                        // other it's the space the wrap consumed.
                        let last = laid[ci].len() == j + 1;
                        let end = match last {
                            true => cell.end,
                            false => line
                                .last()
                                .map(|g| g.src + g.ch.len_utf8())
                                .unwrap_or(cell.end),
                        };
                        glyphs.extend(synth(&" ".repeat(lead + 1), Color::Default, at));
                        glyphs.extend(line.iter().cloned());
                        glyphs.push(Glyph { ch: ' ', style: Style::default(), src: end, stop: true });
                        glyphs.extend(synth(&" ".repeat(trail), Color::Default, end));
                    }
                    // A ragged row, or a column whose cell ended higher up: pad
                    // it out so the grid stays square.
                    _ => {
                        let at = cell.map(|c| c.end).unwrap_or(fallback);
                        glyphs.extend(synth(&" ".repeat(w + 2), Color::Default, at));
                    }
                }
            }
            glyphs.extend(synth("│", Color::DarkGray, fallback));
            // The row ends where its last stop does. A table row has no gap
            // between its final cell and the border, so inventing an end past
            // that would be a stop with nothing under it.
            let end_src = glyphs
                .iter()
                .rev()
                .find(|g| g.stop)
                .map_or(fallback, |g| g.src);
            self.rows.push(VRow { glyphs, end_src, decoration: false });
        }
    }

    fn inline_children(&self, id: usize, base: Style) -> Vec<Glyph> {
        let mut out = Vec::new();
        for c in self.children(id) {
            self.inline(c, base, &mut out);
        }
        out
    }

    fn inline(&self, id: usize, base: Style, out: &mut Vec<Glyph>) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "str" | "smart_punctuation" => push_text(out, node.text.as_deref().unwrap_or(""), node.span.start, base),
            "soft_break" | "hard_break" | "non_breaking_space" => {
                // A break renders as a real, caret-navigable space — but twig
                // gives it no span of its own (`0..0`), so the offset comes from
                // the text in front of it: one *past* the last glyph, which is
                // the newline the break stands for. Past, not on: sharing the
                // previous glyph's offset would put two stops on one byte, and a
                // caret that can't change offset can't move.
                let src = if node.span.start != 0 {
                    node.span.start
                } else {
                    out.last().map(|g| g.src + g.ch.len_utf8()).unwrap_or(0)
                };
                out.push(Glyph { ch: ' ', style: base, src, stop: true });
            }
            "emph" => self.recurse(id, base.italic(), out),
            "strong" => self.recurse(id, base.bold(), out),
            "mark" => self.recurse(id, base.bg(Color::Yellow).fg(Color::Black), out),
            "insert" => self.recurse(id, base.underline(), out),
            "delete" => self.recurse(id, base.strikethrough(), out),
            "superscript" | "subscript" => self.recurse(id, base, out),
            "verbatim" | "inline_math" => {
                // The interior begins at `content_span.start` — past however many
                // backticks the fence used, which `span.start + 1` only guessed
                // right for a single one. Fall back to that guess if it's absent.
                let at = node.content_span.as_ref().map_or(node.span.start + 1, |c| c.start);
                push_text(out, node.text.as_deref().unwrap_or(""), at, base.fg(Color::Green));
            }
            "link" | "url" | "email" => {
                let style = base.fg(Color::Cyan).underline();
                if self.children(id).is_empty() {
                    push_text(out, node.destination.as_deref().or(node.text.as_deref()).unwrap_or("link"), node.span.start, style);
                } else {
                    self.recurse(id, style, out);
                }
            }
            _ => {
                if self.children(id).is_empty() {
                    if let Some(t) = &node.text {
                        push_text(out, t, node.span.start, base);
                    }
                } else {
                    self.recurse(id, base, out);
                }
            }
        }
    }

    fn recurse(&self, id: usize, style: Style, out: &mut Vec<Glyph>) {
        for c in self.children(id) {
            self.inline(c, style, out);
        }
    }

    /// Word-wrap `glyphs` to the available width and push the visual rows,
    /// prefixing the first with `pf` and the rest with `pc`.
    fn emit_wrapped(&mut self, glyphs: Vec<Glyph>, block_start: usize, pf: &[Glyph], pc: &[Glyph]) {
        // No column budget: emit the whole block as one row and let the frontend
        // wrap it at its own (pixel) width.
        let Some(width) = self.wrap else {
            if glyphs.is_empty() {
                self.push_row(pf.to_vec(), block_start);
            } else {
                self.push_row(concat(pf, &glyphs), block_start);
            }
            return;
        };

        // Split into words (maximal non-space runs), each carrying the space
        // glyph that followed it (so its source offset is preserved).
        let mut words: Vec<(Vec<Glyph>, Option<Glyph>)> = Vec::new();
        let mut word: Vec<Glyph> = Vec::new();
        for g in glyphs {
            if g.ch == ' ' {
                words.push((std::mem::take(&mut word), Some(g)));
            } else {
                word.push(g);
            }
        }
        if !word.is_empty() {
            words.push((word, None));
        }
        if words.is_empty() {
            // An empty block still occupies one (prefixed) row.
            self.push_row(pf.to_vec(), block_start);
            return;
        }

        let mut line: Vec<Glyph> = Vec::new();
        let mut used = 0usize;
        let mut first = true;
        for (w, space) in words {
            let avail = width
                .saturating_sub(prefix_width(if first { pf } else { pc }))
                .max(1);
            let cells = glyphs_width(&w);
            if used > 0 && used + cells > avail {
                let row = concat(if first { pf } else { pc }, &line);
                self.push_row(row, block_start);
                line = Vec::new();
                used = 0;
                first = false;
            }
            used += cells;
            line.extend(w);
            if let Some(sp) = space {
                used += 1;
                line.push(sp);
            }
        }
        let row = concat(if first { pf } else { pc }, &line);
        self.push_row(row, block_start);
    }

    /// The source offset of each line of a code block's `text`.
    ///
    /// `content` is the block's `content_span` — where twig says the body lives
    /// in the source, fences already excluded. Its lines run 1:1 with the
    /// rendered `text` lines, so no search is needed; each is anchored at the
    /// *end* of its source line, which places it past whatever indent `text` had
    /// stripped (a fenced block's fences, an indented one's leading spaces)
    /// without having to know how much there was.
    ///
    /// `None` when the body and the rendered lines don't line up — a coarse
    /// fallback the caller turns into the block's start offset.
    fn code_line_offsets(&self, content: &Range<usize>, lines: &[&str]) -> Option<Vec<usize>> {
        let mut src_lines: Vec<(usize, &str)> = Vec::new();
        let mut at = content.start;
        for l in self.source.get(content.start..content.end)?.split('\n') {
            src_lines.push((at, l));
            at += l.len() + 1;
        }
        if src_lines.len() != lines.len() {
            return None;
        }
        Some(
            lines
                .iter()
                .zip(&src_lines)
                .map(|(l, (start, sl))| start + sl.len().saturating_sub(l.len()))
                .collect(),
        )
    }

    fn push_row(&mut self, glyphs: Vec<Glyph>, fallback: usize) {
        let end_src = glyphs
            .last()
            .map(|g| g.src + g.ch.len_utf8())
            .unwrap_or(fallback);
        self.push_row_at(glyphs, end_src);
    }

    /// Push a row with an explicit end stop, for content that knows its own
    /// extent better than its last glyph does.
    fn push_row_at(&mut self, glyphs: Vec<Glyph>, end_src: usize) {
        self.last_off = end_src;
        self.rows.push(VRow { glyphs, end_src, decoration: false });
    }

    /// The source offset the caret rests at on the blank line separating a block
    /// that ends at `prev_end` from the next block starting at `next_start`:
    /// just past the newline that terminates the previous block, but kept
    /// strictly before the next block so the offset is unique to this row.
    fn blank_line_offset(&self, prev_end: usize, next_start: usize) -> usize {
        let after_nl = self.source[prev_end..]
            .find('\n')
            .map_or(prev_end, |p| prev_end + p + 1);
        after_nl.min(next_start.saturating_sub(1)).max(prev_end)
    }

    /// The source offset of each blank row between a block ending at `prev_end`
    /// and content starting at `next_start` — one per blank source line. The
    /// first newline terminates the previous block's line; every line it opens up
    /// to (but not including) the line that holds `next_start` is a blank row the
    /// caret can occupy. Offsets are unique and ascending so `pos_of_offset`
    /// resolves each to its own row. Empty when the two blocks are tight (no
    /// blank line between them).
    fn blank_rows_between(&self, prev_end: usize, next_start: usize) -> Vec<usize> {
        // Spans aren't always in tidy source order (e.g. a block after
        // frontmatter can start *before* the previous block's rendered content
        // ends). There's no blank line to place then — fall back to the clamped
        // single separator (an empty return) rather than slicing an inverted
        // range.
        if next_start <= prev_end {
            return Vec::new();
        }
        let gap = &self.source[prev_end..next_start];
        let Some(nl) = gap.find('\n') else {
            return Vec::new();
        };
        // The line holding `next_start` belongs to the next block; blank rows
        // stop before it.
        let next_line_start = self.source[..next_start]
            .rfind('\n')
            .map_or(0, |p| p + 1);
        let mut offs = Vec::new();
        let mut start = prev_end + nl + 1;
        while start < next_line_start {
            offs.push(start);
            match self.source[start..next_start].find('\n') {
                Some(k) => start += k + 1,
                None => break,
            }
        }
        offs
    }

    /// Blank lines the user typed past the end of the last block (e.g. two
    /// `Enter`s to open a fresh paragraph) leave no AST node, so nothing renders
    /// and the caret appears stuck on the old line. Reconstruct one empty row
    /// per extra trailing newline from the source, each at its own offset, so
    /// the caret rides down onto the new line the moment it's created.
    fn emit_trailing_blank_lines(&mut self) {
        let last_end = self.rows.last().map_or(0, |r| r.end_src);
        if last_end >= self.source.len() {
            return;
        }
        // The first newline after the last content just terminates that line, so
        // a lone trailing `\n` (an ordinary file ending) opens no blank row. A
        // *second* newline opens an empty paragraph: render it the way a block
        // boundary is rendered — a blank spacer row, then the empty paragraph row
        // the caret rests on — so the just-pressed-Enter view already shows the
        // gap it will keep once text is typed, and typing doesn't shift the line
        // down. One row per trailing newline (each its own caret offset), the
        // last landing at the document end where the caret sits.
        let extra = self.source[last_end..].matches('\n').count();
        if extra < 2 {
            return;
        }
        for k in 1..=extra {
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src: last_end + k,
                // As between two blocks: the first blank row is the gap that
                // closes the block above, not somewhere to type. Nothing follows
                // to need a gap of its own, though, so every row after it is a
                // real empty paragraph — the end of the document bounds the last
                // one the way a following block would.
                decoration: k == 1,
            });
        }
    }
}

// ── display width ────────────────────────────────────────────────────────────
//
// Two things a row can be counted in, and they are not the same number:
//
//   *glyphs*, one per codepoint — how the text is stored here, and what an
//   index into `VRow::glyphs` means; and
//   *columns*, one per terminal cell — where the text is drawn, and what every
//   `col` in this crate means.
//
// `你` is one glyph in two columns. Counting columns with `glyphs.len()` (or,
// in the source view, `chars().count()`) is the same number only for the ASCII
// that most fixtures are written in, and drifts one cell per wide character
// everywhere else — the caret drawn a column short of the text it types into.
// Everything below converts between the two; nothing else should have to.

/// The display width of `s` in terminal cells.
///
/// Measured per grapheme cluster, because that is the unit a surface advances
/// by: `👨‍👩‍👧` is five codepoints measuring 2 + 0 + 2 + 0 + 2 cells one at a
/// time, but the character they spell is drawn in 2. Both frontends already
/// measure it that way — ratatui asks `unicode-width` per cluster, and the GUI
/// asks its own text system — so the caret only lands where the text is if this
/// agrees with them.
pub fn text_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// One grapheme cluster of a laid-out row: the glyphs that spell it, and the
/// cells it is drawn in.
///
/// The cluster, not the glyph, is what has a width. A row's glyphs are one per
/// codepoint, so an accented letter or an emoji is several of them drawn in one
/// character's worth of cells — the glyph that opens the cluster claims those
/// cells, and the ones continuing it are drawn *inside* them rather than beside
/// them. It's the same cluster the stop table is built on: the opening glyph is
/// the one a caret can rest on, and so the only one whose column it can be
/// drawn at.
struct Cluster {
    /// Index of the glyph that opens it.
    glyph: usize,
    /// The display column it starts at.
    col: usize,
    /// How many cells it is drawn in. Zero for a cluster with no width of its
    /// own (a lone joiner), which therefore sits at no column at all.
    cells: usize,
}

/// Walk a row's glyphs as the clusters they spell, in column order.
fn clusters(glyphs: &[Glyph]) -> Vec<Cluster> {
    let text: String = glyphs.iter().map(|g| g.ch).collect();
    let mut out = Vec::new();
    let (mut glyph, mut col) = (0, 0);
    for cluster in text.graphemes(true) {
        let cells = text_width(cluster);
        out.push(Cluster { glyph, col, cells });
        // One glyph per codepoint, so a cluster spans exactly its own.
        glyph += cluster.chars().count();
        col += cells;
    }
    out
}

/// The display width of a run of glyphs.
fn glyphs_width(glyphs: &[Glyph]) -> usize {
    clusters(glyphs).last().map_or(0, |c| c.col + c.cells)
}

impl VRow {
    /// The row's width in display columns — and so the column of the caret
    /// placed past its last glyph, which is the rightmost column it can occupy.
    fn width(&self) -> usize {
        glyphs_width(&self.glyphs)
    }

    /// The display column glyph `i` is drawn at. Glyphs continuing a cluster
    /// report the column of the glyph that opened it, since that is where they
    /// are drawn; none of them is ever a stop, so no caret is placed by it.
    fn col_of_glyph(&self, i: usize) -> usize {
        clusters(&self.glyphs)
            .iter()
            .rev()
            .find(|c| c.glyph <= i)
            .map_or(0, |c| c.col)
    }

    /// The glyph drawn at display column `col`, or `None` past the row's last
    /// cell.
    ///
    /// A column landing on the *second* cell of a wide glyph resolves to that
    /// glyph: half a character is not a place to be, so clicking either cell of
    /// `你` means `你`, and the caret comes to rest at its start — the column it
    /// would be drawn at anyway. That rule is what makes the mapping invertible:
    /// every offset has one column, and every column has one offset.
    fn glyph_at_col(&self, col: usize) -> Option<usize> {
        clusters(&self.glyphs)
            .into_iter()
            .find(|c| col < c.col + c.cells)
            .map(|c| c.glyph)
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// One laid-out table cell: its rendered text, the source range that text
/// occupies (`start`/`end` are the caret anchors decoration points at), and the
/// column alignment its padding honours.
///
/// `glyphs` is the cell's inline content *unwrapped* — the box-drawn rows wrap
/// it to a column width, but a frontend laying the grid out itself needs the
/// text before that decision was made.
#[derive(Clone)]
pub struct TableCell {
    pub glyphs: Vec<Glyph>,
    pub start: usize,
    pub end: usize,
    pub align: Alignment,
}

/// One row of a table's grid, as the document spells it — not as it's drawn.
#[derive(Clone)]
pub struct TableRow {
    /// A header row: drawn bold, and ruled off from the body below it.
    pub head: bool,
    pub cells: Vec<TableCell>,
}

/// A table's structure, published alongside the box-drawn rows that spell it.
///
/// The rows in [`VisualMap::rows`] are a *terminal's* picture of a table: every
/// border a `│`, every column a whole number of character cells. That picture is
/// exactly right where a cell is a cell, and unfixable anywhere else — in a
/// proportional font the `│`s of two rows land at different x and the grid
/// shears. So a frontend that draws its own geometry reads this instead: the
/// cells, their alignment, and which rows are the head, with no opinion about
/// how wide a column is or what a border looks like.
///
/// Both are always built. The TUI paints `rows` and ignores this; the GUI skips
/// `rows` for the span in `rows_span` and draws from here. They describe the
/// same cells, so the caret lands on the same offsets either way.
#[derive(Clone)]
pub struct TableInfo {
    /// The `VisualMap::rows` this table's picture occupies, borders included —
    /// what a frontend drawing its own table skips over.
    pub rows_span: Range<usize>,
    /// The source span of the table node, and the offset its trailing caret
    /// stop sits at.
    pub end_src: usize,
    /// The block prefix every row of this table carries — a blockquote's `│ `
    /// gutter, a list item's indent. Empty for a table at the top level.
    ///
    /// A frontend drawing its own grid has to render this and start the table
    /// past it, exactly as the picture does; a table nested in a quote that
    /// draws flush at the left margin has left the quote.
    pub prefix: Vec<Glyph>,
    pub grid: Vec<TableRow>,
}

/// The narrowest a column may be squeezed. Below a few characters a column
/// stops carrying text and just shreds it one letter per line, which is worse
/// than letting the grid run wide.
const MIN_COL_WIDTH: usize = 3;

/// Shrink `widths` until the grid fits `avail` screen columns, taking from the
/// widest column each time so the loss is shared out rather than falling on
/// whichever column happens to be last. No column goes below
/// [`MIN_COL_WIDTH`]; a table with more columns than the surface has room for
/// still overflows, which is the honest outcome — there's nothing left to give.
fn fit_widths(widths: &mut [usize], avail: usize) {
    // Chrome: each column is its content plus a gutter either side, and every
    // column is closed by a `│` — with one more opening the row.
    let budget = avail.saturating_sub(3 * widths.len() + 1);
    while widths.iter().sum::<usize>() > budget {
        let Some(w) = widths.iter_mut().filter(|w| **w > MIN_COL_WIDTH).max() else {
            return;
        };
        *w -= 1;
    }
}

/// Word-wrap `glyphs` into lines of at most `width` columns, hard-breaking any
/// single word too long to fit.
///
/// Unlike a paragraph — where an overlong word just trails off the end of the
/// line — a table column is a hard boundary: a glyph past it lands on top of
/// the border, or on the next cell. So the width here is a promise, and a word
/// that won't keep it is broken.
///
/// The space at a break is dropped rather than hung past the edge. Its offset
/// isn't lost: the caller gives every line an end stop just past its last
/// glyph, which is exactly where that space was.
///
/// `width` is in display columns, and a break only ever falls between grapheme
/// clusters. Both matter to more than the picture: the caller anchors each
/// line's end stop just past its last glyph, so a line cut mid-cluster would
/// put a caret stop inside a character — reachable by Down or a click, and the
/// next Backspace would take the cluster apart from the middle.
fn wrap_glyphs(glyphs: &[Glyph], width: usize) -> Vec<Vec<Glyph>> {
    let width = width.max(1);
    // Words are maximal non-space runs, each carrying the space that followed it
    // — which survives only if the next word joins it on this line.
    let mut words: Vec<(Vec<Glyph>, Option<Glyph>)> = Vec::new();
    let mut word: Vec<Glyph> = Vec::new();
    for g in glyphs {
        if g.ch == ' ' {
            words.push((std::mem::take(&mut word), Some(g.clone())));
        } else {
            word.push(g.clone());
        }
    }
    if !word.is_empty() {
        words.push((word, None));
    }

    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    let mut line: Vec<Glyph> = Vec::new();
    let mut used = 0usize;
    let mut gap: Option<Glyph> = None;
    for (word, space) in words {
        for chunk in hard_break(&word, width) {
            let sep = gap.is_some() as usize;
            let cells = glyphs_width(chunk);
            if !line.is_empty() && used + sep + cells > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
                gap = None; // the break swallows the space
            }
            if let Some(sp) = gap.take() {
                line.push(sp);
                used += 1;
            }
            line.extend_from_slice(chunk);
            used += cells;
        }
        gap = space;
    }
    // An empty cell is still one (empty) line — it has an end the caret can
    // sit at, which is how you type into it.
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Break a single word into pieces of at most `width` columns, cutting only
/// between grapheme clusters — the replacement for slicing it into fixed runs
/// of glyphs, which measures a wide character as one column and can cut an
/// emoji in half.
///
/// A cluster wider than the whole column still gets a piece to itself: there is
/// nowhere legal to cut it, and overflowing by a cell is better than splitting a
/// character. An empty word yields no pieces at all, which is what keeps a
/// double space from opening a line of its own.
fn hard_break(word: &[Glyph], width: usize) -> Vec<&[Glyph]> {
    let mut out = Vec::new();
    if word.is_empty() {
        return out;
    }
    let (mut start, mut used) = (0usize, 0usize);
    for c in clusters(word) {
        if used > 0 && used + c.cells > width {
            out.push(&word[start..c.glyph]);
            start = c.glyph;
            used = 0;
        }
        used += c.cells;
    }
    out.push(&word[start..]);
    out
}

/// A table rule spanning `widths`, e.g. `┌──────┬─────┐`. Each column is its
/// content width plus the one-space gutter on either side.
fn rule_text(widths: &[usize], left: char, mid: char, right: char) -> String {
    let mut s = String::new();
    s.push(left);
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            s.push(mid);
        }
        for _ in 0..w + 2 {
            s.push('─');
        }
    }
    s.push(right);
    s
}

/// Push real document text: each glyph maps to its own source byte, and the one
/// that opens a grapheme cluster is the caret stop for the whole cluster.
///
/// Per cluster rather than per codepoint because a cluster is the character the
/// user sees, and it's the unit backspace and delete already step by. A stop
/// inside 👨‍👩‍👧 — five codepoints strung together with joiners — is a caret
/// parked in the middle of a character: one press of Right lands there, and the
/// next Backspace severs a joiner from what it joined, leaving a dangling ZWJ in
/// the source. The rest of the cluster still gets its glyph (it has to be
/// drawn); it just isn't somewhere to stand.
fn push_text(out: &mut Vec<Glyph>, text: &str, base_src: usize, style: Style) {
    for (gi, cluster) in text.grapheme_indices(true) {
        for (ci, ch) in cluster.char_indices() {
            out.push(Glyph { ch, style, src: base_src + gi + ci, stop: ci == 0 });
        }
    }
}

/// Build synthetic prefix glyphs (a bullet, a gutter) all pointing at `src`.
/// `Color::Default` yields the surface's own color (no override). Synthetic
/// glyphs are never caret stops — they share one offset, so the caret steps
/// over them (a click still lands at `src`).
fn synth(text: &str, color: Color, src: usize) -> Vec<Glyph> {
    let style = Style::default().fg(color);
    text.chars()
        .map(|ch| Glyph { ch, style, src, stop: false })
        .collect()
}

fn concat(a: &[Glyph], b: &[Glyph]) -> Vec<Glyph> {
    let mut v = a.to_vec();
    v.extend_from_slice(b);
    v
}

/// The columns a row's prefix (a bullet, a quote gutter, an indent) takes up
/// before the text it introduces — what the wrap budget has left to spend.
fn prefix_width(prefix: &[Glyph]) -> usize {
    glyphs_width(prefix)
}

fn heading_style(level: u32) -> Style {
    let base = Style::default().bold();
    match level {
        1 => base.fg(Color::Cyan).underline(),
        2 => base.fg(Color::Green),
        3 => base.fg(Color::Yellow),
        4 => base.fg(Color::Blue),
        5 => base.fg(Color::Magenta),
        _ => base.fg(Color::Gray),
    }
}

pub(crate) fn is_inline(kind: &str) -> bool {
    matches!(
        kind,
        "str" | "soft_break" | "hard_break" | "non_breaking_space" | "emph" | "strong" | "mark"
            | "insert" | "delete" | "verbatim" | "inline_math" | "display_math" | "url" | "email"
            | "link" | "image" | "smart_punctuation" | "superscript" | "subscript" | "span"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use twig::{Editor, Format};

    fn map(src: &str) -> VisualMap {
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        build(&ed.nodes().unwrap(), src, Some(80))
    }

    fn rendered(m: &VisualMap) -> String {
        m.rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn resolves_markup_to_plain_text() {
        let text = rendered(&map("# Title\n\na **bold** word\n"));
        assert!(!text.contains('#'), "heading marker shown: {text:?}");
        assert!(!text.contains("**"), "strong delimiters shown: {text:?}");
        assert!(text.contains("Title") && text.contains("bold word"));
    }

    #[test]
    fn every_glyph_points_at_its_source_byte() {
        let src = "a **bold** c\n";
        let m = map(src);
        for row in &m.rows {
            for g in &row.glyphs {
                // A real (non-synthetic) glyph's source byte is the glyph's char.
                if g.src < src.len() && src.is_char_boundary(g.src) {
                    if let Some(sc) = src[g.src..].chars().next() {
                        if sc == g.ch {
                            continue;
                        }
                    }
                }
                // Synthetic prefixes (none here) would be the only exceptions.
                panic!("glyph {:?} at src {} doesn't match source", g.ch, g.src);
            }
        }
    }

    #[test]
    fn offset_and_position_round_trip_on_visible_text() {
        let m = map("hello world\n");
        let (r, c) = m.pos_of_offset(6); // the 'w'
        assert_eq!(m.offset_of_pos(r, c), 6);
    }

    #[test]
    fn unwrapped_mode_emits_one_row_per_paragraph() {
        // A long paragraph that would wrap under a column budget stays a single
        // row when wrap is None (the GUI wraps it at pixel width instead).
        let long = "one two three four five six seven eight nine ten eleven twelve\n";
        let mut ed = Editor::new_str(long, Format::Markdown).unwrap();
        let wrapped = build(&ed.nodes().unwrap(), long, Some(12));
        let unwrapped = build(&ed.nodes().unwrap(), long, None);
        assert!(wrapped.num_rows() > 1, "narrow column should wrap");
        assert_eq!(unwrapped.num_rows(), 1, "no budget should keep it one row");
        // Every glyph's source byte is preserved in the single row.
        let text: String = unwrapped.rows[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(text.trim_end(), long.trim_end());
    }

    #[test]
    fn an_empty_paragraph_between_blocks_renders_its_own_rows() {
        // "A", then two blank lines (an empty paragraph opened with Enter), then
        // "B": the empty paragraph must be navigable rows, not collapsed onto B.
        // Rows: "A", spacer, empty-paragraph, spacer, "B" — each blank row a
        // distinct source offset.
        let m = map("A\n\n\n\nB\n");
        let text: Vec<String> = m
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect();
        assert_eq!(text, vec!["A", "", "", "", "B"], "got {text:?}");
        let offs: Vec<usize> = m.rows.iter().map(|r| r.end_src).collect();
        // Strictly ascending — no two rows share an offset (else the caret pins).
        assert!(offs.windows(2).all(|w| w[0] < w[1]), "offsets not unique: {offs:?}");
    }

    #[test]
    fn a_tight_block_boundary_still_gets_one_separator() {
        // A heading directly above text (no blank line between) keeps the single
        // conventional separator row, as before.
        let m = map("# H\ntext\n");
        let text: Vec<String> = m
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect();
        assert_eq!(text, vec!["H", "", "text"], "got {text:?}");
    }

    #[test]
    fn frontmatter_is_hidden_and_the_document_opens_into_its_content() {
        // Leading YAML frontmatter renders nothing — no phantom blank rows for
        // its lines, no leading gap — and `content_start` points at the first
        // real block so the caret floor can keep out of the hidden metadata.
        let fm = "---\nconfig: colophon.yaml\ncontents:\n- '[Sample](sample.md)'\n---\n";
        let src = format!("{fm}# leaf\n\nA line.\n");
        let m = map(&src);
        let text = rendered(&m);
        assert!(!text.contains("config"), "frontmatter body leaked: {text:?}");
        assert!(!text.contains("colophon"), "frontmatter body leaked: {text:?}");
        assert_eq!(m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(), "leaf");
        assert_eq!(m.content_start, fm.len(), "floor should be the first real block");
    }

    #[test]
    fn a_document_without_frontmatter_has_a_zero_floor() {
        let m = map("# leaf\n\nbody\n");
        assert_eq!(m.content_start, 0);
    }

    /// `| Name | Qty |` with Name left-aligned and Qty right-aligned.
    const TABLE: &str = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n| Fig | 12 |\n";

    #[test]
    fn a_table_renders_as_an_aligned_grid() {
        let text = rendered(&map(TABLE));
        assert_eq!(
            text,
            "┌──────┬─────┐\n\
             │ Name │ Qty │\n\
             ├──────┼─────┤\n\
             │ Pear │   3 │\n\
             │ Fig  │  12 │\n\
             └──────┴─────┘",
            "got:\n{text}"
        );
    }

    #[test]
    fn table_columns_honour_their_alignment() {
        // Centre and default(left) come straight from twig's cell.alignment —
        // the delimiter row it's spelled in is consumed and has no node.
        let text = rendered(&map("| A | Bee |\n| --- | :---: |\n| x | y |\n"));
        assert!(text.contains("│ x │  y  │"), "centred column: {text:?}");
    }

    #[test]
    fn table_borders_are_decoration_the_caret_never_lands_on() {
        let m = map(TABLE);
        // The rules are whole decoration rows.
        for r in [0, 2, 5] {
            assert!(m.rows[r].decoration, "row {r} should be a decoration rule");
            assert!(!m.rows[r].glyphs.iter().any(|g| g.stop), "row {r} has a stop");
        }
        // A content row's `│` and padding are decoration; only the cell text
        // and each cell's one end-stop are stops.
        let header = &m.rows[1];
        assert!(!header.decoration);
        for g in &header.glyphs {
            if g.ch == '│' {
                assert!(!g.stop, "a border is not a caret stop");
            }
        }
        let stops: String = header.glyphs.iter().filter(|g| g.stop).map(|g| g.ch).collect();
        assert_eq!(stops, "Name Qty ", "cell text plus one end-stop space each");
    }

    #[test]
    fn a_cell_maps_to_its_own_source_text() {
        let m = map(TABLE);
        // "Pear" starts at byte 32 in TABLE; the caret there draws on the 'P'.
        let pear = TABLE.find("Pear").unwrap();
        let (r, c) = m.pos_of_offset(pear);
        assert_eq!(m.rows[r].glyphs[c].ch, 'P');
        assert_eq!(m.offset_of_pos(r, c), pear, "round trips");
    }

    #[test]
    fn a_wide_table_is_cut_to_fit_and_its_cells_wrap() {
        // Columns wider than the surface used to run off the right edge, where
        // nothing could reach them. They're cut to the budget instead, and the
        // text wraps down inside the column — the header rule stays put, and
        // an alignment holds on every line of a wrapped cell, not just the first.
        let src = "| Ingredient | Notes |\n|---|---:|\n\
                   | flour milled coarse | sift it twice |\n| salt | a pinch |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build(&ed.nodes().unwrap(), src, Some(30));
        let text = rendered(&m);
        assert_eq!(
            text,
            "┌──────────────┬─────────────┐\n\
             │ Ingredient   │       Notes │\n\
             ├──────────────┼─────────────┤\n\
             │ flour milled │     sift it │\n\
             │ coarse       │       twice │\n\
             │ salt         │     a pinch │\n\
             └──────────────┴─────────────┘",
            "got:\n{text}"
        );
        for (r, row) in m.rows.iter().enumerate() {
            assert!(row.glyphs.len() <= 30, "row {r} overflows: {}", row.glyphs.len());
        }
    }

    #[test]
    fn a_column_too_narrow_for_a_word_breaks_it_rather_than_spilling() {
        // A paragraph lets an overlong word trail off the end of the line; a
        // table column can't — a glyph past the border lands on the border.
        let src = "| A | B |\n|---|---|\n| antidisestablishmentarianism | x |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build(&ed.nodes().unwrap(), src, Some(20));
        for (r, row) in m.rows.iter().enumerate() {
            assert!(row.glyphs.len() <= 20, "row {r} overflows: {}", row.glyphs.len());
        }
        // Broken across lines, but whole: every letter is still drawn, at its
        // own source byte, where the caret can reach it.
        let word = "antidisestablishmentarianism";
        let at = src.find(word).unwrap();
        for (i, ch) in word.char_indices() {
            assert!(
                m.rows
                    .iter()
                    .flat_map(|r| r.glyphs.iter())
                    .any(|g| g.stop && g.src == at + i && g.ch == ch),
                "{ch:?} at {} was lost to the break", at + i
            );
        }
    }

    #[test]
    fn a_code_block_maps_each_line_to_its_own_source_text() {
        // Every glyph used to point at the block's start, which made the whole
        // block one offset — visible, but impossible to put a caret inside.
        let src = "```rust\nlet x = 1;\nfn f() {}\n```\n";
        let m = map(src);
        for row in &m.rows {
            for g in row.glyphs.iter().filter(|g| g.stop) {
                assert_eq!(
                    src[g.src..].chars().next(),
                    Some(g.ch),
                    "glyph {:?} at {} isn't the source byte it claims",
                    g.ch,
                    g.src
                );
            }
        }
    }

    #[test]
    fn an_indented_code_block_maps_past_its_stripped_indent() {
        // twig strips the four-space indent, so `text` isn't a source slice and
        // the lines have to be re-found. Offsets land on the code, not the indent.
        let src = "    indented\n    code\n";
        let m = map(src);
        let stops: Vec<(char, usize)> = m
            .rows
            .iter()
            .flat_map(|r| r.glyphs.iter().filter(|g| g.stop).map(|g| (g.ch, g.src)))
            .collect();
        assert_eq!(stops[0], ('i', 4), "first line should start past the indent");
        assert!(stops.contains(&('c', 17)), "second line misplaced: {stops:?}");
    }

    #[test]
    fn a_fenced_block_whose_code_echoes_its_info_string_maps_to_the_code() {
        // The one case that defeats a forward search: the opening fence
        // ```` ```rust ```` ends with the same text as the code under it.
        let src = "```rust\nrust\n```\n";
        let m = map(src);
        let first = m.rows[0].glyphs.iter().find(|g| g.stop).unwrap();
        assert_eq!(first.src, 8, "matched the info string, not the code");
    }

    #[test]
    fn caret_steps_over_hidden_delimiters() {
        // "a **bold** c": bytes 8,9 are the closing ** — no glyph. Moving right
        // from 'd' (src 7) lands on the space before 'c' (src 10), not inside **.
        let m = map("a **bold** c\n");
        let (r, c) = m.pos_of_offset(7);
        assert_eq!(m.offset_of_pos(r, c + 1), 10);
    }

    // ── the structural view of a table ───────────────────────────────────────

    #[test]
    fn a_table_is_published_structurally_beside_its_picture() {
        let m = map(TABLE);
        let t = &m.tables[0];
        let cell = |r: usize, c: usize| -> String {
            t.grid[r].cells[c].glyphs.iter().map(|g| g.ch).collect()
        };
        assert_eq!(t.grid.len(), 3, "head + two body rows");
        assert_eq!(
            (cell(0, 0), cell(0, 1), cell(1, 0), cell(2, 1)),
            ("Name".into(), "Qty".into(), "Pear".into(), "12".into())
        );
        assert_eq!(
            t.grid.iter().map(|r| r.head).collect::<Vec<_>>(),
            [true, false, false]
        );
        // The alignment the delimiter row spelled, carried per cell — the only
        // place it survives, since the parser consumes that row.
        assert!(matches!(t.grid[1].cells[0].align, Alignment::Left));
        assert!(matches!(t.grid[1].cells[1].align, Alignment::Right));
    }

    #[test]
    fn the_structural_table_spans_exactly_its_drawn_rows() {
        // A frontend drawing its own grid skips `rows_span` and renders from
        // `grid`. If the span were short the leftover border rows would be
        // painted as text under the real table; if long it would eat a
        // neighbouring paragraph. Both are silent, so pin it to the picture.
        let m = map(&format!("before\n\n{TABLE}\nafter\n"));
        let t = &m.tables[0];
        let row_text = |r: usize| -> String { m.rows[r].glyphs.iter().map(|g| g.ch).collect() };
        assert!(row_text(t.rows_span.start).starts_with('┌'), "opens on the top border");
        assert!(
            row_text(t.rows_span.end - 1).starts_with('└'),
            "closes on the bottom border"
        );
        assert!(
            !row_text(t.rows_span.start - 1).contains('┌'),
            "the row before the span is not the table's"
        );
        assert_eq!(row_text(t.rows_span.end), "", "the span ends before the gap row");
    }

    #[test]
    fn a_nested_tables_structure_carries_the_block_prefix() {
        // The picture puts the quote's gutter on every row of the grid. A
        // frontend drawing its own table has to draw that too and start past it,
        // so the prefix has to travel with the structure — without it a quoted
        // table renders flush at the margin and leaves the quote it's in.
        let m = map("> | a | b |\n> |---|---|\n> | c | d |\n");
        let t = &m.tables[0];
        let prefix: String = t.prefix.iter().map(|g| g.ch).collect();
        assert_eq!(prefix, "│ ", "the quote's gutter should ride the structure");
        // And it matches what the picture actually drew.
        let drawn: String = m.rows[t.rows_span.start].glyphs.iter().map(|g| g.ch).collect();
        assert!(drawn.starts_with(&prefix), "picture and structure disagree: {drawn:?}");
    }

    #[test]
    fn a_top_level_table_carries_no_prefix() {
        assert!(map(TABLE).tables[0].prefix.is_empty());
    }

    #[test]
    fn structural_cells_are_unwrapped_even_when_the_picture_wraps_them() {
        // The picture wraps a cell to its column; a frontend laying the grid out
        // in pixels needs the text as the document spells it, before that
        // decision. Narrow enough that the drawn cell must break.
        let src = "| Name |\n|------|\n| alpha beta gamma |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build(&ed.nodes().unwrap(), src, Some(12));
        let drawn = rendered(&m);
        let cell: String = m.tables[0].grid[1].cells[0]
            .glyphs
            .iter()
            .map(|g| g.ch)
            .collect();
        assert_eq!(cell, "alpha beta gamma", "structure must not carry the wrap");
        assert!(
            drawn.lines().count() > 5,
            "the picture should have wrapped, else this proves nothing:\n{drawn}"
        );
    }

    // ── display columns ──────────────────────────────────────────────────────

    #[test]
    fn a_table_column_is_as_wide_as_its_cells_are_drawn() {
        // A column sized by counting characters is drawn narrower than the text
        // it has to hold — `你好` is two characters in four cells — and the cell
        // spills over the border it is supposed to sit inside, taking the whole
        // grid out of square with it. Squareness is the property: every row of a
        // grid is drawn to the same column, whatever its cells are spelled with.
        for src in [
            "| A | B |\n|---|---|\n| 你好 | y |\n",
            "| A | B |\n|---|---|\n| a👨‍👩‍👧b | y |\n",
            "| A | 漢字 |\n|---|---|\n| x | y |\n",
        ] {
            let m = map(src);
            let widths: Vec<usize> = m.rows.iter().map(|r| r.width()).collect();
            assert!(
                widths.windows(2).all(|w| w[0] == w[1]),
                "ragged grid {widths:?} for {src:?}:\n{}",
                rendered(&m)
            );
        }
    }

    #[test]
    fn a_cell_wrapped_narrow_never_breaks_inside_a_character() {
        // A column too narrow for its cell hard-breaks the text, and every line
        // of it is given an end stop just past its last glyph. Broken into runs
        // of four glyphs, the first line of this cell ends between `👨‍👩` and the
        // joiner holding `👧` on — so its end stop lands inside a character,
        // where a click or Down can reach it and the next Backspace takes the
        // cluster apart from the middle.
        let src = "| A |\n|---|\n| 👨‍👩‍👧👨‍👩‍👧 |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build(&ed.nodes().unwrap(), src, Some(8));
        let boundaries: Vec<usize> = src
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(src.len()))
            .collect();
        for off in (0..=src.len()).filter(|&o| m.is_stop(o)) {
            assert!(
                boundaries.contains(&off),
                "stop at {off} is inside a character:\n{}",
                rendered(&m)
            );
        }
    }

    #[test]
    fn a_wrapped_cell_keeps_every_line_inside_its_column() {
        // The width is a promise in a table, where a glyph past the column lands
        // on the border or in the next cell — and it is a promise about cells,
        // which is not what a count of glyphs measures.
        let src = "| A |\n|---|\n| 你好世界漢字 |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build(&ed.nodes().unwrap(), src, Some(14));
        for r in &m.rows {
            assert_eq!(r.width(), 14, "{:?} is not drawn to the grid", rendered(&m));
        }
    }

    #[test]
    fn a_hard_break_falls_between_clusters_and_measures_in_cells() {
        let glyphs = |s: &str| {
            let mut out = Vec::new();
            push_text(&mut out, s, 0, Style::default());
            out
        };
        let piece = |p: &[Glyph]| p.iter().map(|g| g.ch).collect::<String>();

        // Six cells of CJK broken at four: two characters, then one — never
        // between the two cells of `好`.
        let w = glyphs("你好世");
        let pieces: Vec<String> = hard_break(&w, 4).iter().map(|p| piece(p)).collect();
        assert_eq!(pieces, ["你好", "世"]);

        // A character wider than the column has nowhere legal to break, so it
        // keeps its cells rather than being cut in half.
        let w = glyphs("你好");
        let pieces: Vec<String> = hard_break(&w, 1).iter().map(|p| piece(p)).collect();
        assert_eq!(pieces, ["你", "好"]);

        // An empty word yields no pieces at all — a double space stays a space.
        assert!(hard_break(&[], 4).is_empty());
    }

    #[test]
    fn every_caret_stop_opens_a_cluster_of_its_row() {
        // The two ways of finding a cluster have to agree. `push_text` marks the
        // stops by segmenting one run of text; the column mapping segments the
        // whole row, decoration and all. A stop that came out as the *middle* of
        // some row-level cluster would be a caret with no column of its own —
        // drawn at the column of whatever swallowed it.
        let src = "# 標題\n\na **bold** e\u{0301}mo👨‍👩‍👧ji `x` 你好\n\n\
                   - 項目 one\n- e\u{0301}dge\n\n> 引用 text\n\n\
                   | A | 值 |\n|---|---|\n| 你好 | 👩‍🚀 |\n";
        let m = map(src);
        for (r, row) in m.rows.iter().enumerate() {
            let openers: Vec<usize> = clusters(&row.glyphs).iter().map(|c| c.glyph).collect();
            for (i, g) in row.glyphs.iter().enumerate() {
                assert!(
                    !g.stop || openers.contains(&i),
                    "row {r}: the stop at glyph {i} ({:?}) is inside a cluster, \
                     so it is drawn at another glyph's column",
                    g.ch
                );
            }
        }
    }
}

