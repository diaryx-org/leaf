//! Tables, drawn as an aligned grid of wrapped cells.

use super::*;

impl Builder<'_> {
    /// Render a table as a box-drawn grid: every column as wide as its widest
    /// cell, the header bold and ruled off, each cell padded to its column's
    /// alignment. This is the *default* monospace rendering (see
    /// [`VisualMap::rows`]); the same cells are also published structurally as
    /// [`TableInfo`], so a frontend that lays the grid out in its own units draws
    /// from there and skips the picture built here.
    ///
    /// The alignment comes from twig's `cell.alignment` — the delimiter row
    /// (`|:--|--:|`) that spells it out is consumed by the parser and leaves no
    /// node, so the snapshot is the only source for it.
    ///
    /// Borders and padding are *decoration*: they carry the source offset of the
    /// text they surround, so a click lands in that cell, but they're never
    /// caret stops — the caret steps cell-to-cell instead of into the box art.
    pub(super) fn table(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node_end = self.nodes[id].span.end;
        // twig's shape is `[caption, row, row, …]`: the caption is always
        // present (usually empty in Markdown) and is not part of the grid.
        let row_ids: Vec<usize> = self
            .children(id)
            .into_iter()
            .filter(|&c| self.nodes[c].kind == Kind::Row)
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
                widths[c] = widths[c].max(cell_width(&cell.glyphs));
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
        // The bottom border is the one rule the caret can rest on: its end is
        // the table's trailing stop, the caret home just past the block — the
        // peer of a block picture's second stop, and of a rule's row end. Without
        // it a document ending in a table ended *inside* it: nothing after the
        // last cell was a stop, so Right could not leave the table, and a click
        // in the blank space under it snapped back into the last cell — or, on a
        // surface that resolved the click onto the border row, to the table's
        // first cell, since a decoration row's only stop is the nearest one.
        // Typing at the stop opens a paragraph first, as at a picture's — see
        // `Doc::open_paragraph_at_block_edge`. The glyphs stay non-stops at
        // `node_end`, so a click anywhere on the border lands past the table.
        self.push_rule_with_home(&rule_text(&widths, '└', '┴', '┘'), node_end, pc);

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
        // A cell is one source line, so a break within it is an explicit line
        // break (an inline `<br>`) that must render as a line of its own — not the
        // flow-folding space a break is in prose.
        self.break_glyph.set('\n');
        let cells = self
            .children(row)
            .into_iter()
            .filter(|&c| self.nodes[c].kind == Kind::Cell)
            .enumerate()
            .map(|(col, c)| {
                let n = &self.nodes[c];
                let style = if n.head.unwrap_or(false) {
                    Style::default().bold()
                } else {
                    Style::default()
                };
                // Only `content_span` bounds a cell's text, and an EMPTY cell
                // has none at all — twig records no interior for it — so both
                // offsets would fall back to the cell's `span.start`: on the
                // pipe that opens it, or (under a twig that gave every cell
                // the whole row's span) the row's start, where every empty
                // cell collapses onto one spot before the first `│` and a
                // caret there types *before* the table. Derive the interior
                // from the span's own pipes and this cell's column instead,
                // so each empty cell has a distinct, editable caret home.
                let span = n.content_span.clone().unwrap_or_else(|| {
                    let off = empty_cell_offset(
                        &self.source[n.span.start.min(self.source.len())
                            ..n.span.end.min(self.source.len())],
                        n.span.start,
                        col,
                    );
                    off..off
                });
                TableCell {
                    glyphs: self.inline_children(c, style),
                    start: span.start,
                    end: span.end,
                    align: n.alignment.unwrap_or(Alignment::Default),
                }
            })
            .collect();
        self.break_glyph.set(' ');
        cells
    }

    /// A horizontal rule between/around rows — entirely decoration.
    fn push_rule(&mut self, text: &str, src: usize, prefix: &[Glyph]) {
        self.push_rule_row(text, src, prefix, true);
    }

    /// A table's bottom border: drawn like the other rules, but a row the caret
    /// can rest on, its end (`src`) being the table's trailing stop.
    fn push_rule_with_home(&mut self, text: &str, src: usize, prefix: &[Glyph]) {
        self.push_rule_row(text, src, prefix, false);
    }

    fn push_rule_row(&mut self, text: &str, src: usize, prefix: &[Glyph], decoration: bool) {
        let glyphs = concat(prefix, &synth(text, Role::Rule, src));
        self.rows.push(VRow {
            glyphs,
            end_src: src,
            decoration,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            media: None,
            task: None,
            leaf_directive: None,
            heading: None,
            align: None,
            line_height: None,
            hang: None,
            boundary: None,
            mark_ends: Vec::new(),
            math: Vec::new(),
            region: None,
            inline_regions: Vec::new(),
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
                glyphs.extend(synth("│", Role::Rule, at));
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
                        glyphs.extend(synth(&" ".repeat(lead + 1), Role::Body, at));
                        glyphs.extend(line.iter().cloned());
                        glyphs.push(Glyph {
                            ch: ' ',
                            style: Style::default(),
                            src: end,
                            stop: true,
                        });
                        glyphs.extend(synth(&" ".repeat(trail), Role::Body, end));
                    }
                    // A ragged row, or a column whose cell ended higher up: pad
                    // it out so the grid stays square.
                    _ => {
                        let at = cell.map(|c| c.end).unwrap_or(fallback);
                        glyphs.extend(synth(&" ".repeat(w + 2), Role::Body, at));
                    }
                }
            }
            glyphs.extend(synth("│", Role::Rule, fallback));
            // The row ends where its last stop does. A table row has no gap
            // between its final cell and the border, so inventing an end past
            // that would be a stop with nothing under it.
            let end_src = glyphs
                .iter()
                .rev()
                .find(|g| g.stop)
                .map_or(fallback, |g| g.src);
            let mark_ends = self.take_mark_ends(end_src);
            let math = self.take_math(&glyphs);
            let inline_regions = self.take_inline_regions(&glyphs);
            self.rows.push(VRow {
                glyphs,
                end_src,
                decoration: false,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                align: None,
                line_height: None,
                hang: None,
                boundary: None,
                mark_ends,
                math,
                region: None,
                inline_regions,
            });
        }
    }
}
