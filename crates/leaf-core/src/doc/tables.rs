//! Tables: structural operations on the caret's table and the keys that
//! navigate between its cells.

use super::*;

impl Doc {
    // ── Tables ───────────────────────────────────────────────────────────────
    // A table is a grid, and twig edits it as one — add/remove/move a row or
    // column, set a column's alignment — re-spelling the whole table in a single
    // splice. Every gesture is anchored at the caret's cell. leaf just names the
    // gesture and re-reads the result; the whole table's numbering, borders, and
    // delimiter are twig's to keep straight.

    /// Whether the caret is inside a table — what a frontend asks to enable or
    /// disable its table controls.
    ///
    /// An HTML `<table>` still answers `true`: the caret really is in a table,
    /// and the reason the grid controls stay dark there is
    /// [`Capabilities::table`], which is a fact about the document's format
    /// rather than about the caret. A frontend needs both.
    pub fn caret_in_table(&mut self) -> bool {
        let caret = self.caret.min(self.source.len());
        self.editor
            .ancestors_at(caret)
            .map(|c| c.into_iter().any(|m| m.kind == Kind::Table))
            .unwrap_or(false)
    }

    /// Every table cell, in source order: its text, the bytes a
    /// [`crate::TableCell`]'s `start..end` spans, an empty cell's the one
    /// caret home core derives for it; and its table, row and column. Read
    /// off the parse, not the visual map, so a host that draws the document
    /// its own way (Leaf's pages), and builds no map between keystrokes, has
    /// it for the source it is setting.
    pub fn table_cells(&mut self) -> Vec<CellSpan> {
        // A pipe table spells a `|`; a document without one has no grid to
        // walk the whole tree for.
        if !self.source.contains('|') && !self.source.contains("<t") {
            return Vec::new();
        }
        let nodes = self.nodes();
        let len = self.source.len();
        let children = |id: Option<twig::NodeId>| {
            std::iter::successors(id.and_then(|i| nodes.get(i.0 as usize)), |n| {
                n.next_sibling.and_then(|i| nodes.get(i.0 as usize))
            })
        };
        let mut out = Vec::new();
        for table in nodes.iter().filter(|n| n.kind == Kind::Table) {
            let rows = children(table.first_child).filter(|n| n.kind == Kind::Row);
            for (row, r) in rows.enumerate() {
                let cells = children(r.first_child).filter(|n| n.kind == Kind::Cell);
                for (col, n) in cells.enumerate() {
                    let text = n.content_span.clone().unwrap_or_else(|| {
                        let (start, end) = (n.span.start.min(len), n.span.end.min(len));
                        let at =
                            crate::wysiwyg::empty_cell_offset(&self.source[start..end], start, col);
                        at..at
                    });
                    out.push(CellSpan {
                        text,
                        table: table.span.start,
                        row,
                        col,
                    });
                }
            }
        }
        out.sort_by_key(|c| (c.text.start, c.text.end));
        out
    }

    /// One grid op, guarded and settled — the shared body of the seven below.
    ///
    /// The guard is why this exists rather than seven copies of the same three
    /// lines, and it is the one guard leaf cannot delegate to twig. The table
    /// editor is the gesture family that consults no `Syntax` table (it spells a
    /// grid, not a delimiter) and therefore the one twig's `Format::supports`
    /// deliberately has no variant for: handed an HTML `<table>` it rebuilds the
    /// grid as a *pipe table* and reports success, swapping the element out for
    /// `| a | b |` and taking the rest of the document's markup with it. Nothing
    /// downstream could tell that from a successful edit — the splice is real,
    /// the reparse succeeds, `dirty` is honest — which is what makes it worth
    /// stopping at the door rather than detecting after the fact. See
    /// [`spells_pipe_tables`].
    fn table_op(
        &mut self,
        what: &str,
        op: impl FnOnce(&mut Editor, usize) -> Result<(), twig::Error>,
    ) {
        if self.refuse_unless(what, spells_pipe_tables(self.format)) {
            return;
        }
        self.record_caret();
        let at = self.caret;
        let r = op(&mut self.editor, at);
        self.apply_table(r, what);
    }

    /// Insert an empty row below (`below`) or above the caret's row.
    pub fn table_insert_row(&mut self, below: bool) {
        self.table_op("table row", |e, at| e.table_insert_row(at, below));
    }

    /// Delete the caret's row (not the header, not the last body row).
    pub fn table_delete_row(&mut self) {
        self.table_op("table row", |e, at| e.table_delete_row(at));
    }

    /// Insert an empty column right (`right`) or left of the caret's column.
    pub fn table_insert_column(&mut self, right: bool) {
        self.table_op("table column", |e, at| e.table_insert_column(at, right));
    }

    /// Delete the caret's column (unless it is the only one).
    pub fn table_delete_column(&mut self) {
        self.table_op("table column", |e, at| e.table_delete_column(at));
    }

    /// Set the caret's column to `alignment`.
    pub fn table_set_alignment(&mut self, alignment: Alignment) {
        self.table_op("table alignment", |e, at| {
            e.table_set_alignment(at, alignment)
        });
    }

    /// Move the caret's row one place down (`down`) or up, within the body rows.
    pub fn table_move_row(&mut self, down: bool) {
        self.table_op("table row", |e, at| e.table_move_row(at, down));
    }

    /// Move the caret's column one place right (`right`) or left.
    pub fn table_move_column(&mut self, right: bool) {
        self.table_op("table column", |e, at| e.table_move_column(at, right));
    }

    /// Settle the caret and document flags after a table op (or report its
    /// error). twig re-spells the whole table, so the caret rides its old byte
    /// offset and is clamped back into the rebuilt bytes — near enough to where
    /// it was, since the op preserves the cells' content and order around it.
    fn apply_table(&mut self, result: Result<(), twig::Error>, what: &str) {
        match result {
            Ok(()) => {
                self.last_edit_kind = None;
                self.refresh();
                self.anchor = None;
                self.clamp_caret();
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{what}: {e}")),
        }
    }
}

impl Doc {
    // ── table key policy ──────────────────────────────────────────────────────
    // The three keys a table gives its own meaning — Tab, Return, Shift+Return —
    // as one policy every frontend shares, rather than each re-deriving it. Each
    // reports whether it acted *as a table key*; a `false` hands the key back to
    // the frontend's ordinary handling (indent, newline) so it keeps its meaning
    // everywhere else.

    /// Tab / Shift+Tab inside a table. Tab steps to the next cell, appending a
    /// fresh row and entering it when it runs off the last one; Shift+Tab steps
    /// back and simply stays put at the very first cell. `false` when the caret
    /// isn't in a table.
    pub fn cell_tab(&mut self, forward: bool) -> bool {
        if !self.caret_in_table() {
            return false;
        }
        if self.cell_hop(forward) {
            return true;
        }
        // Off the last cell: grow the table by a row and step into its first
        // cell. (Shift+Tab at the first cell has nowhere to go and just holds.)
        if forward {
            self.append_row_and_enter(0);
        }
        true
    }

    /// Return inside a table: drop to the cell below in the same column,
    /// appending a new row when the caret is already in the last one. `false`
    /// when the caret isn't in a table, so the frontend inserts a newline.
    pub fn cell_return(&mut self) -> bool {
        if !self.caret_in_table() {
            return false;
        }
        if self.cell_move_vertical(true) {
            return true;
        }
        // Already on the last row: grow one below and drop into the same column.
        let col = self.table_grid_at(self.caret).map_or(0, |(_, _, c)| c);
        self.append_row_and_enter(col);
        true
    }

    /// Append a row below the caret's (last) row and land in `col` of it. The
    /// caret is in the last row, so twig's "insert below" makes the fresh row the
    /// table's new last — but twig re-spells the whole table, moving every byte,
    /// so the destination is read back from the rebuilt grid by the table's
    /// position (stable across a row insert), not from the pre-edit caret.
    fn append_row_and_enter(&mut self, col: usize) {
        let table = self.caret_table_index();
        self.table_insert_row(true);
        self.rebuild_map();
        let Some((start, end)) = table
            .and_then(|ti| self.vmap.tables.get(ti))
            .and_then(|t| t.grid.last())
            .and_then(|row| row.cells.get(col.min(row.cells.len().saturating_sub(1))))
            .map(|cell| (cell.start, cell.end))
        else {
            return;
        };
        self.select_cell(start, end);
    }

    /// The index, among the document's tables, of the one the caret sits in —
    /// `None` when it's in none. Used to re-find a table after an edit re-spells
    /// it (a row insert leaves the table order unchanged).
    fn caret_table_index(&self) -> Option<usize> {
        let off = self.caret;
        self.vmap.tables.iter().position(|t| {
            t.grid
                .iter()
                .any(|row| row.cells.iter().any(|c| off >= c.start && off <= c.end))
        })
    }

    /// Shift+Return inside a table: insert a hard line break *within* the current
    /// cell, via twig's `insert_line_break`. `false` when the caret isn't in a
    /// table, so the frontend inserts an ordinary line break.
    ///
    /// A table row is a single source line, so the newline-spelled hard break
    /// can't live in a cell. twig spells the in-cell break the format's way
    /// (`<br>` for Markdown) and reparses it as a *semantic* `hard_break`, so the
    /// break round-trips as structure the renderer reads back as a line — not the
    /// opaque raw HTML the old raw-splice left behind.
    ///
    /// Djot has no idiomatic in-cell break, so twig refuses it
    /// (`UnsupportedFormat`) rather than emit a `<br>` that any other djot reader
    /// would render as the literal text `<br>`. The gesture is still *consumed*
    /// there — returning `false` would let the frontend insert a real newline,
    /// which splits the one-line row — it just leaves the cell unchanged and says
    /// so on the status line. A rollback (`EditConflict`) is swallowed the same.
    ///
    /// Which formats refuse is [`Capabilities::cell_line_break`], and the two
    /// have to be read together: djot is not the only `false`, and naming it in
    /// the message was already a guess that HTML — which spells the break as its
    /// own `<br>` — would have made wrong.
    pub fn cell_line_break(&mut self) -> bool {
        if self.read_only || !self.caret_in_table() {
            return false;
        }
        self.record_caret();
        match self.editor.insert_line_break(self.caret) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                self.caret = change.new.end;
                self.anchor = None;
                self.goal_col = None;
                self.clamp_caret();
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
            }
            Err(twig::Error::UnsupportedFormat) => {
                self.status = Some(format!(
                    "in-cell line breaks aren't supported in {}",
                    self.format_name()
                ));
            }
            Err(_) => {}
        }
        true
    }
}
