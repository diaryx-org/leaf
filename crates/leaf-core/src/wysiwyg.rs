//! The WYSIWYG view: render the document with its markup *resolved*, not shown —
//! headings coloured, `**bold**` as real bold, `# ` / `**` / `` ` `` delimiters
//! hidden — while keeping every visible glyph tied back to the source byte it
//! came from.
//!
//! That back-reference (`Glyph::src`) is what lets a caret still work: the caret
//! stays a source offset (shared with the source view), but the [`VisualMap`]
//! converts between an offset and a screen `(row, col)`, so cursor drawing,
//! mouse clicks, and — crucially — arrow motion all operate in *visible* space
//! and step right over the hidden delimiters.
//!
//! Text is walked from the AST (`str` nodes carry exact spans, and their text is
//! the verbatim source slice), so a Markdown and a Djot file that parse alike
//! render — and map — identically.

use twig::{Alignment, FlatNode};

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
    pub stop: bool,
}

/// One visual line. `end_src` is the source offset a caret sits at when placed
/// at the line's end (past its last glyph) — the anchor for end-of-line and
/// click-past-content.
pub struct VRow {
    pub glyphs: Vec<Glyph>,
    pub end_src: usize,
    /// A row that is *entirely* decoration (a table's `├───┼───┤` rules): it
    /// holds no caret stop, so vertical motion steps over it and
    /// `pos_of_offset` never resolves onto it. Distinct from a row that merely
    /// has no glyphs — a blank paragraph gap is empty but is a real caret stop.
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
}

impl VisualMap {
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn row_len(&self, row: usize) -> usize {
        self.rows.get(row).map(|r| r.glyphs.len()).unwrap_or(0)
    }

    /// The screen `(row, col)` for a source offset — where to draw the caret.
    /// Snaps a hidden offset (inside a delimiter) to the next visible glyph, and
    /// never resolves onto decoration (a table border, a cell's padding), which
    /// is drawn but holds no caret.
    pub fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        for (r, row) in self.rows.iter().enumerate() {
            if row.decoration {
                continue;
            }
            for (c, g) in row.glyphs.iter().enumerate() {
                if g.stop && g.src >= off {
                    return (r, c);
                }
            }
            if row.end_src >= off {
                return (r, row.glyphs.len());
            }
        }
        let r = self.last_stop_row();
        (r, self.row_len(r))
    }

    /// The source offset for a screen `(row, col)` — where a click or a
    /// visual-space move lands the caret. Clicking decoration maps through its
    /// `src`, which points at the text it decorates, so a click on a border or
    /// on a cell's padding lands in that cell.
    pub fn offset_of_pos(&self, row: usize, col: usize) -> usize {
        let Some(r) = self.rows.get(row) else {
            return 0;
        };
        match r.glyphs.get(col) {
            Some(g) => g.src,
            None => r.end_src,
        }
    }

    /// Whether the caret can occupy `row` at all: decoration rows (a table's
    /// border rules) are stepped over by vertical motion.
    pub fn row_is_navigable(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| !r.decoration)
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

    /// The offset of the nearest caret stop strictly before `(row, col)` — one
    /// press of Left. Steps over a run of decoration in one go rather than
    /// stalling on padding that all shares a single offset, and falls to the end
    /// of the row above when there's nothing left on this one.
    pub fn stop_before(&self, row: usize, col: usize) -> Option<usize> {
        if let Some(vr) = self.rows.get(row) {
            if !vr.decoration {
                let upto = col.min(vr.glyphs.len());
                if let Some(g) = vr.glyphs[..upto].iter().rev().find(|g| g.stop) {
                    return Some(g.src);
                }
            }
        }
        let above = self.navigable_above(row)?;
        Some(self.rows[above].end_src)
    }

    /// The offset of the nearest caret stop strictly after `(row, col)` — one
    /// press of Right, falling to the start of the row below once this row is
    /// spent.
    pub fn stop_after(&self, row: usize, col: usize) -> Option<usize> {
        if let Some(vr) = self.rows.get(row) {
            // `col == glyphs.len()` means the caret is already at the row's end
            // stop, so the only way on is the next row.
            if !vr.decoration && col < vr.glyphs.len() {
                if let Some(g) = vr.glyphs[col + 1..].iter().find(|g| g.stop) {
                    return Some(g.src);
                }
                // Past the last glyph the row's end is itself a stop — but only
                // where it's a *distinct* offset. A table row ends at its last
                // cell's end, an offset that cell's own end stop already holds,
                // so there's nothing past the closing `│` to land on.
                let last = vr.glyphs.iter().rev().find(|g| g.stop).map(|g| g.src);
                if vr.end_src > last.unwrap_or(0) {
                    return Some(vr.end_src);
                }
            }
        }
        let below = self.navigable_below(row)?;
        let vr = &self.rows[below];
        Some(match vr.glyphs.iter().find(|g| g.stop) {
            Some(g) => g.src,
            None => vr.end_src,
        })
    }
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
        last_off: 0,
    };
    b.blocks(doc, &[], &[]);
    b.emit_trailing_blank_lines();
    let content_start = first_content_offset(nodes, doc);
    VisualMap {
        rows: b.rows,
        content_start,
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
                for end_src in offs {
                    self.rows.push(VRow {
                        glyphs: pc.to_vec(),
                        end_src,
                        decoration: false,
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
                    let indent = synth(&" ".repeat(marker.chars().count()), Color::Default, start);
                    self.block(item, &concat(pc, &bullet), &concat(pc, &indent));
                }
            }
            "list_item" | "task_list_item" => self.blocks(id, pf, pc),
            "table" => self.table(id, pf, pc),
            "code_block" => {
                let style = Style::default().fg(Color::Green);
                let text = node.text.clone().unwrap_or_default();
                let base = node.span.start; // coarse: code editing is a source-view job
                for raw in text.trim_end_matches('\n').split('\n') {
                    let gutter = synth("▏ ", Color::DarkGray, base);
                    let mut glyphs: Vec<Glyph> = concat(pf, &gutter);
                    for ch in raw.chars() {
                        // Code text is a caret stop, though the whole block maps
                        // coarsely to `base` (code editing is a source-view job).
                        glyphs.push(Glyph { ch, style, src: base, stop: true });
                    }
                    self.push_row(glyphs, base);
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
                widths[c] = widths[c].max(cell.glyphs.len());
            }
        }

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

    /// One `│ a │ b │` content row: real cell text between decoration.
    fn push_table_row(&mut self, cells: &[TableCell], widths: &[usize], prefix: &[Glyph]) {
        let fallback = cells.last().map(|c| c.end).unwrap_or(0);
        let mut glyphs = prefix.to_vec();
        for (ci, &w) in widths.iter().enumerate() {
            // The divider before this column belongs to the cell it introduces,
            // so clicking it lands at that cell's start.
            let at = cells.get(ci).map(|c| c.start).unwrap_or(fallback);
            glyphs.extend(synth("│", Color::DarkGray, at));
            match cells.get(ci) {
                Some(cell) => {
                    let pad = w.saturating_sub(cell.glyphs.len());
                    let (lead, trail) = match cell.align {
                        Alignment::Right => (pad, 0),
                        Alignment::Center => (pad / 2, pad - pad / 2),
                        Alignment::Left | Alignment::Default => (0, pad),
                    };
                    glyphs.extend(synth(&" ".repeat(lead + 1), Color::Default, cell.start));
                    glyphs.extend(cell.glyphs.iter().cloned());
                    // Every cell renders at least one space after its text (the
                    // gutter before `│`), so there is always somewhere to put
                    // the "after the last character" caret a cell needs just as
                    // a line does. It's the one padding glyph that is a stop.
                    glyphs.push(Glyph {
                        ch: ' ',
                        style: Style::default(),
                        src: cell.end,
                        stop: true,
                    });
                    glyphs.extend(synth(&" ".repeat(trail), Color::Default, cell.end));
                }
                // A ragged row: pad the missing cell out so the grid stays square.
                None => glyphs.extend(synth(&" ".repeat(w + 2), Color::Default, fallback)),
            }
        }
        glyphs.extend(synth("│", Color::DarkGray, fallback));
        let end_src = fallback;
        self.rows.push(VRow {
            glyphs,
            end_src,
            decoration: false,
        });
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
                let src = if node.span.start != 0 { node.span.start } else { out.last().map(|g| g.src).unwrap_or(0) };
                // A break renders as a real, caret-navigable space.
                out.push(Glyph { ch: ' ', style: base, src, stop: true });
            }
            "emph" => self.recurse(id, base.italic(), out),
            "strong" => self.recurse(id, base.bold(), out),
            "mark" => self.recurse(id, base.bg(Color::Yellow).fg(Color::Black), out),
            "insert" => self.recurse(id, base.underline(), out),
            "delete" => self.recurse(id, base.strikethrough(), out),
            "superscript" | "subscript" => self.recurse(id, base, out),
            "verbatim" | "inline_math" => {
                // No content_span; map the interior to just after the opener.
                push_text(out, node.text.as_deref().unwrap_or(""), node.span.start + 1, base.fg(Color::Green));
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
            if used > 0 && used + w.len() > avail {
                let row = concat(if first { pf } else { pc }, &line);
                self.push_row(row, block_start);
                line = Vec::new();
                used = 0;
                first = false;
            }
            used += w.len();
            line.extend(w);
            if let Some(sp) = space {
                used += 1;
                line.push(sp);
            }
        }
        let row = concat(if first { pf } else { pc }, &line);
        self.push_row(row, block_start);
    }

    fn push_row(&mut self, glyphs: Vec<Glyph>, fallback: usize) {
        let end_src = glyphs
            .last()
            .map(|g| g.src + g.ch.len_utf8())
            .unwrap_or(fallback);
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
                decoration: false,
            });
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// One laid-out table cell: its rendered text, the source range that text
/// occupies (`start`/`end` are the caret anchors decoration points at), and the
/// column alignment its padding honours.
struct TableCell {
    glyphs: Vec<Glyph>,
    start: usize,
    end: usize,
    align: Alignment,
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

/// Push real document text: each glyph maps to its own source byte, so every
/// one of them is a caret stop.
fn push_text(out: &mut Vec<Glyph>, text: &str, base_src: usize, style: Style) {
    for (i, ch) in text.char_indices() {
        out.push(Glyph { ch, style, src: base_src + i, stop: true });
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

fn prefix_width(prefix: &[Glyph]) -> usize {
    prefix.len()
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
    fn caret_steps_over_hidden_delimiters() {
        // "a **bold** c": bytes 8,9 are the closing ** — no glyph. Moving right
        // from 'd' (src 7) lands on the space before 'c' (src 10), not inside **.
        let m = map("a **bold** c\n");
        let (r, c) = m.pos_of_offset(7);
        assert_eq!(m.offset_of_pos(r, c + 1), 10);
    }
}

