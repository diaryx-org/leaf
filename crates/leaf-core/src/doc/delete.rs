//! Backspace and forward delete, by character, word, or to a line's edge, and
//! the block joins they can cause.

use super::*;

impl Doc {
    pub fn backspace(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
            return;
        }
        // WYSIWYG: Backspace at the very start of a list item's content is a
        // structural key, not a character delete — it walks the "un-indent, then
        // un-list" ladder every list editor gives that keystroke (outdent a
        // nested item, strip a top-level one's marker to a paragraph). In source
        // view the `- ` is visible text the user is deleting a byte of, so it
        // keeps its literal meaning there, like Enter does.
        if self.view != View::Source && self.backspace_list_start() {
            return;
        }
        // WYSIWYG: and the same at the start of a heading's content — the `# `
        // there is markup the rich view hides, not text the user typed.
        if self.view != View::Source && self.backspace_heading_start() {
            return;
        }
        // WYSIWYG: and at the start of a block whose presentation is spelled
        // as hidden markup before it — djot's `{.center}` line, Markdown's
        // `<div class="center">` — Backspace takes that markup, the way it
        // takes a heading's `#`, rather than a byte out of it.
        if self.view != View::Source && self.backspace_attributed_block_start() {
            return;
        }
        // WYSIWYG: at a block picture's stops, a byte-at-a-time delete would take
        // the markup apart under a caret that cannot see it — see
        // `delete_around_block_atom`.
        if self.view != View::Source && self.delete_around_block_atom(false) {
            return;
        }
        // WYSIWYG: Backspace at a table's trailing stop steps back into its last
        // cell rather than taking the byte behind the caret — the row's closing
        // `|`, which the rich view never drew, so the key would have looked like
        // it did nothing. The stop before is the last cell's end.
        if self.view != View::Source && self.backspace_at_table_end() {
            return;
        }
        // WYSIWYG: a table cell's start is a wall. The bytes behind it are the
        // padding and the `|` that make the grid, and taking them merges two
        // cells — a structural edit nobody asked for, and one no table editor
        // gives the key. Tab and Shift+Tab are how the caret leaves a cell.
        if self.view != View::Source && self.at_cell_wall(BreakEdge::Backward) {
            return;
        }
        // WYSIWYG: at the start of a block's content, the byte behind the caret
        // is a block boundary, and Backspace over one is a join — twig's, so
        // that what a join is in each format is not this file's to know. After
        // the picture and table cases, which are block starts with their own
        // answers.
        // WYSIWYG: in a verse, Backspace at a line's start joins it to the
        // line above — the text editor's join, taking the `\` that broke the
        // line with the newline — and at the first line's start turns the verse
        // back into prose, the way it un-lists a list. See `backspace_verse`.
        if self.view != View::Source && self.backspace_verse() {
            return;
        }
        if self.view != View::Source && self.backspace_joins_block() {
            return;
        }
        // WYSIWYG: Backspace on a *blank line* deletes back to the previous caret
        // stop, not a single newline. On a line with no text of its own, the byte
        // before the caret is a `\n` that spells part of a block boundary — the gap
        // between two blocks, drawn but never a caret home. Removing just it strands
        // the caret in that gap and leaves an odd blank line the eye reads as one
        // separator but the caret can't land on: the "extra newline" left behind
        // after leaving a list (Enter, Enter) or a paragraph and pressing Backspace.
        // Deleting to the previous stop instead collapses the whole break at once,
        // landing the caret at the end of the block above. Two blank lines in a row
        // are one stop apart, so this still removes exactly one — the lone-Enter /
        // lone-Backspace symmetry the empty-line case is built on is untouched.
        if self.view != View::Source
            && self.caret > self.caret_floor()
            && self.caret_on_blank_line()
            && let Some(stop) = self.vmap.stop_before(self.caret)
        {
            let stop = stop.max(self.caret_floor());
            if stop < self.caret {
                if self.source[stop..self.caret].trim().is_empty() {
                    self.splice(stop, self.caret, "", EditKind::Delete);
                } else {
                    // Hidden markup stands between the stop and the caret — a
                    // `</div>`, a comment, a link reference definition — and
                    // collapsing to the stop would delete it. Take the blank
                    // line alone, with the newline that opened it, and land
                    // the caret where the collapse would have.
                    self.delete_blank_line_to(stop);
                }
                return;
            }
        }
        if self.caret > self.caret_floor() {
            // An in-cell `<br>` draws as one newline glyph, so Backspace over it
            // takes the whole tag — a single-byte step would leave a broken `<br`
            // showing in the cell. Rich view only (source view edits the literal).
            if self.view != View::Source
                && let Some((start, end)) = self.cell_break_at(BreakEdge::Backward)
            {
                let start = start.max(self.caret_floor());
                if start < end {
                    self.splice(start, end, "", EditKind::Delete);
                    return;
                }
            }
            // Aim the delete at the character the writer can *see* behind the
            // caret, never at a delimiter the rich view drew nothing for. Two
            // steps, and either can apply: from the far side of a run's closing
            // `**` step back into the run (the caret is drawn at the end of its
            // word), and at the start of a run's text step out past its opening
            // `**` to the character in front of it, leaving the run standing.
            // Without them a plain Backspace unspells the phrase it is editing
            // and leaves a literal asterisk on screen.
            let end = if self.view == View::Source {
                self.caret
            } else {
                let inside = self.step_inside_close_delims(self.caret);
                // An attributed span with no text — `<span …></span>` as the
                // file was written — is hidden markup around nothing, and a
                // byte-step here would take its `>`. Backspace takes the span
                // whole, with the character before it: the character the key
                // looks aimed at, since the span draws nothing.
                if let Some(span) = self.run_span_of_content(inside..inside) {
                    let from = if self.source[..span.start].ends_with('\n') {
                        span.start
                    } else {
                        prev_boundary(&self.source, span.start)
                    };
                    let from = from.max(self.caret_floor());
                    self.splice(from, span.end, "", EditKind::Delete);
                    return;
                }
                self.skip_leading_open_delims(inside)
                    .max(self.caret_floor())
            };
            // Never delete back across the floor — that would eat hidden
            // frontmatter the WYSIWYG caret can't even see.
            let mut prev = prev_boundary(&self.source, end).max(self.caret_floor());
            // Take a hidden escape backslash with the char it escapes: the rich
            // view draws `\*` as a single `*`, so Backspace over it must delete
            // both bytes, never strand the `\` as a lone visible backslash (the
            // mirror of the Hidden-mode typing that wrote the escape). Source view
            // shows the `\`, so there it is an ordinary character.
            if self.view != View::Source
                && prev > self.caret_floor()
                && self.is_hidden_escape(prev - 1)
            {
                prev -= 1;
            }
            // The delete that takes the last of a span's text takes the span
            // with it, in the same edit: `<span …>i</span>` losing its `i`
            // would leave an empty span the map has no stop inside, so the
            // caret would draw at the next stop — a line away — until a
            // further key removed the span. Landing on the span's start is
            // where the letter was.
            if self.view != View::Source
                && let Some(span) = self.run_span_of_content(prev..end)
            {
                self.splice(span.start, span.end, "", EditKind::Delete);
                return;
            }
            // And the same for a block: the letter that was all of a centred
            // paragraph's text goes with the `<div>` around it, or the `{…}`
            // line above it, leaving a plain blank line where the letter was.
            // A paragraph with no text is no block, so the markup would stand
            // around nothing, the map would give it no caret home, and the
            // next key would take the tag apart.
            if self.view != View::Source
                && let Some(block) = self.attributed_block_of_content(prev..end)
            {
                self.splice(block.start, block.end, "", EditKind::Delete);
                return;
            }
            if prev < end {
                self.splice(prev, end, "", EditKind::Delete);
            }
        }
    }

    /// Remove the blank line the caret is on — its own newline and the one
    /// that ended the line before it — and put the caret on `stop`, the caret
    /// stop before it. The [`backspace`](Self::backspace) blank-line rule for a
    /// blank line that hidden markup separates from the block above: the
    /// navigable blank row after a `</div>` is always one of at least three
    /// newlines under the tag (the drawn separators either side of it), so
    /// taking two leaves the blank line the tag needs under it.
    fn delete_blank_line_to(&mut self, stop: usize) {
        let caret = self.caret;
        let line_start = self.source[..caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[caret..]
            .find('\n')
            .map_or(self.source.len(), |i| caret + i);
        let from = line_start.saturating_sub(1).max(stop);
        let to = (line_end + 1).min(self.source.len());
        self.splice(from, to, "", EditKind::Delete);
        self.caret = stop;
        self.anchor = None;
        self.goal_col = None;
        self.record_caret();
    }

    /// Move the caret to the stop before it and consume the key — what
    /// Backspace does where the byte behind the caret is hidden markup it
    /// has no structural answer for, rather than take that markup apart.
    fn step_back_to_stop(&mut self) {
        // The map answers about offsets, so it has to be this revision's — see
        // `open_paragraph_at_block_edge`.
        self.rebuild_map();
        if let Some(off) = self
            .vmap
            .stop_before(self.caret)
            .filter(|&o| o >= self.caret_floor())
        {
            self.caret = off;
            self.anchor = None;
            self.goal_col = None;
        }
    }

    /// Backspace's presentation behaviour: with the caret exactly at the start
    /// of a block's content, and that block's attributes spelled as hidden
    /// markup before it, strip the attributes. The peer of
    /// [`backspace_heading_start`](Self::backspace_heading_start), and the same
    /// reasoning: the `{.center}` line above a djot block and the
    /// `<div class="center">` around a Markdown one are what the byte behind
    /// the caret belongs to, and the rich view draws neither. The ordinary
    /// delete took the newline out of `{.center}\nhello` and left
    /// `{.center}hello` — the attribute line fused onto the text as prose —
    /// and out of `<div …>\n\nhello` it took the blank line the div needs.
    ///
    /// The whole attribute set goes, the way the whole `#` marker does — the
    /// press is over the line that spells it, not over one key of it — and
    /// twig's `set_block_attrs` with an empty list is the edit: it removes the
    /// djot line and unwraps the Markdown div. Where the block is the first of
    /// several in a div, twig has no sole child to unwrap and answers with a
    /// no-op, so the caret steps back to the stop before instead, as it does
    /// at a table's end. A later child of the div has an ordinary paragraph
    /// above it and is not this rule's.
    ///
    /// Returns whether it acted; `false` leaves Backspace its character delete.
    fn backspace_attributed_block_start(&mut self) -> bool {
        if !matches!(self.format, Format::Markdown | Format::Djot) {
            return false;
        }
        let caret = self.caret;
        let nodes = self.nodes();
        let Some(block) = nodes
            .iter()
            .filter(|n| matches!(n.kind, Kind::Para | Kind::Heading))
            .find(|n| n.content_span.as_ref().map_or(n.span.start, |c| c.start) == caret)
        else {
            return false;
        };
        match self.format {
            Format::Djot => {
                // twig records where the `{…}` block was written, so this is
                // the parser's own answer and not a scan for a `{` above the
                // block; `None` (a synthesized or merged set) is not a line
                // the caret is standing after.
                let spelled = self
                    .editor
                    .document()
                    .ok()
                    .and_then(|mut d| d.attrs_span(block.id).ok().flatten())
                    .is_some_and(|s| s.end <= caret);
                if !spelled {
                    return false;
                }
            }
            _ => {
                let Some(div) = block
                    .parent
                    .and_then(|p| nodes.iter().find(|n| n.id == p))
                    .filter(|p| wysiwyg::element_tag(p) == Some("div"))
                else {
                    return false;
                };
                let mut kids = nodes.iter().filter(|n| n.parent == Some(div.id));
                if kids.clone().any(|k| k.span.start < block.span.start) {
                    return false;
                }
                if kids.nth(1).is_some() {
                    self.step_back_to_stop();
                    return true;
                }
            }
        }
        self.write_block_attrs("block attributes", Vec::new());
        true
    }

    /// Backspace at the start of a block's content: join the block into the
    /// block before it, as one gesture — twig's `join_blocks`, the inverse of
    /// the split Enter makes, spelled the format's way. Two paragraphs join
    /// on a soft break; a paragraph under a marker heading joins onto the
    /// heading's line; a paragraph after a Markdown `<div>` moves inside it,
    /// the hidden `</div>` carried past the joined text; HTML's `</p><p>` is
    /// taken as one; a quote's or an item's continuation prefix is written.
    /// The joined text takes the block above's presentation and containers,
    /// which is the rule every editor with a centred paragraph follows.
    ///
    /// Leaf used to join by deleting the one newline behind the caret, which
    /// is the right bytes for two Markdown paragraphs and nothing else: under
    /// a heading it left two blocks, in HTML it took the `>` off a tag, and
    /// after a div it took the newline under the hidden `</div>`, which drew
    /// nothing different and took the tag apart on the next press. What a
    /// join is in each format is twig's to know, and now it does.
    ///
    /// Where twig refuses — the block above is a code block, a table or a
    /// rule with no text to join into, or the caret's block would have to
    /// leave a div that holds more after it — the caret steps back to the
    /// stop before instead, as it does at a table's end: the key moves the
    /// caret and takes no markup apart. Where nothing precedes the block, or
    /// the format cannot join at all, Backspace keeps its character delete.
    ///
    /// Returns whether it acted.
    fn backspace_joins_block(&mut self) -> bool {
        let caret = self.caret;
        if caret <= self.caret_floor() {
            return false;
        }
        let Some(text) = self.text_block_opening_at(caret) else {
            return false;
        };
        match self.join_blocks(caret) {
            Ok(change) => {
                // The caret keeps its place at the start of the text it stood
                // on, wherever the join put that text — after a soft break,
                // a space, or a quote's prefix. Found by the bytes, as
                // `block_content_in` finds a re-spelled block.
                let region = &self.source[change.new.clone()];
                let at = region
                    .find(&text)
                    .map_or(change.new.start, |i| change.new.start + i);
                self.land_after_join(at);
                true
            }
            Err(twig::Error::NotEditable) => {
                self.step_back_to_stop();
                true
            }
            Err(twig::Error::NotFound | twig::Error::UnsupportedFormat) => false,
            Err(e) => {
                self.status = Some(format!("join: {e}"));
                true
            }
        }
    }

    /// Delete at the end of a block's content: join the block after it into
    /// this one — [`backspace_joins_block`](Self::backspace_joins_block)'s
    /// mirror, and the same twig gesture aimed at the next block. The caret
    /// stays where it was, which is where the joined text now begins after
    /// the separator. Where twig refuses, the caret steps forward to the next
    /// stop instead; where no block follows, Delete keeps its character
    /// delete.
    fn delete_forward_joins_block(&mut self) -> bool {
        let caret = self.caret;
        let at_end = self
            .nodes()
            .iter()
            .filter(|n| matches!(n.kind, Kind::Para | Kind::Heading))
            .any(|n| n.content_span.as_ref().is_some_and(|c| c.end == caret));
        if !at_end {
            return false;
        }
        // The next stop, across a line end, in a block: what Delete at a
        // block's end points at. On the same line it is a hidden delimiter's
        // far side, which the ordinary delete handles; on a blank line it is
        // the empty paragraph the byte delete has always closed.
        self.rebuild_map();
        let Some(stop) = self.vmap.stop_after(caret) else {
            return false;
        };
        if !self.source[caret..stop].contains('\n') || !self.has_block_at(stop) {
            return false;
        }
        match self.join_blocks(stop) {
            Ok(change) => {
                self.land_after_join(change.old.start);
                true
            }
            Err(twig::Error::NotEditable | twig::Error::NotFound) => {
                self.caret = stop;
                self.anchor = None;
                self.goal_col = None;
                true
            }
            Err(twig::Error::UnsupportedFormat) => false,
            Err(e) => {
                self.status = Some(format!("join: {e}"));
                true
            }
        }
    }

    /// The content bytes of the paragraph or heading whose content opens
    /// exactly at `off` — the block a Backspace there is at the start of.
    fn text_block_opening_at(&mut self, off: usize) -> Option<String> {
        self.nodes()
            .into_iter()
            .filter(|n| matches!(n.kind, Kind::Para | Kind::Heading))
            .find_map(|n| {
                let c = n.content_span?;
                (c.start == off).then(|| self.source[c].to_string())
            })
    }

    /// Hand the block at `offset` to twig's `join_blocks`, with the undo
    /// plumbing every structural gesture has; the caret is the caller's to
    /// place from the change, via [`land_after_join`](Self::land_after_join).
    fn join_blocks(&mut self, offset: usize) -> Result<Change, twig::Error> {
        if self.read_only {
            return Err(twig::Error::NotEditable);
        }
        self.record_caret();
        let change = self.editor.join_blocks(offset)?;
        self.last_edit_kind = None; // structural edit is its own undo step
        self.refresh();
        Ok(change)
    }

    /// Finish a join: the caret at `at`, no selection, the map this
    /// revision's before the clamp — see `write_block_attrs` for why.
    fn land_after_join(&mut self, at: usize) {
        self.caret = at;
        self.anchor = None;
        self.goal_col = None;
        self.dirty = self.source != self.clean_source;
        self.status = None;
        self.rebuild_map();
        self.clamp_caret();
        self.record_caret();
    }
}

impl Doc {
    /// Backspace at a table's trailing stop: move onto the stop before it (the
    /// last cell's end) and consume the key. `false` anywhere else. See
    /// [`VisualMap::table_end_stop`] for why the byte behind the caret there is
    /// not one to delete.
    fn backspace_at_table_end(&mut self) -> bool {
        // The map answers about offsets, so it has to be this revision's — see
        // `open_paragraph_at_block_edge`.
        self.rebuild_map();
        if !self.vmap.table_end_stop(self.caret) {
            return false;
        }
        if let Some(off) = self
            .vmap
            .stop_before(self.caret)
            .filter(|&o| o >= self.caret_floor())
        {
            self.caret = off;
            self.anchor = None;
            self.goal_col = None;
        }
        true
    }

    /// Whether the caret stands at a table cell's wall on the `edge` side — at
    /// or before its first caret stop for Backspace, at or past its last for
    /// Delete — where the only bytes between it and the neighbouring cell are
    /// the padding and the `|` the rich view draws as the grid. The padding
    /// counts as the cell's: a caret placed between `| ` and the text is at the
    /// same wall, so the first press cannot eat the space either. An in-cell
    /// `<br>` at the edge is not a wall; the delete takes it whole, as ever.
    fn at_cell_wall(&mut self, edge: BreakEdge) -> bool {
        // The map answers about offsets, so it has to be this revision's.
        self.rebuild_map();
        if self.cell_break_at(edge).is_some() {
            return false;
        }
        let caret = self.caret;
        let src = self.source.as_bytes();
        let pad = |b: &u8| *b == b' ' || *b == b'\t';
        self.vmap
            .tables
            .iter()
            .flat_map(|t| &t.grid)
            .flat_map(|row| &row.cells)
            .any(|cell| {
                let lo = cell.start
                    - src[..cell.start]
                        .iter()
                        .rev()
                        .take_while(|b| pad(b))
                        .count();
                let hi = cell.end + src[cell.end..].iter().take_while(|b| pad(b)).count();
                (lo..=hi).contains(&caret)
                    && match edge {
                        BreakEdge::Backward => {
                            self.vmap.stop_before(caret).is_none_or(|s| s < cell.start)
                        }
                        BreakEdge::Forward => {
                            self.vmap.stop_after(caret).is_none_or(|s| s > cell.end)
                        }
                    }
            })
    }

    /// Whether the caret's own source line holds nothing but whitespace — an
    /// empty paragraph, or the blank line a block boundary is spelled with. The
    /// test for [`backspace`](Self::backspace)'s stop-wise delete: such a line has
    /// no text of its own, so the newline before the caret belongs to the gap
    /// between blocks rather than to any word the caret is editing.
    fn caret_on_blank_line(&self) -> bool {
        let line_start = self.source[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[self.caret..]
            .find('\n')
            .map_or(self.source.len(), |i| self.caret + i);
        self.source[line_start..line_end].trim().is_empty()
    }

    /// The source span of an in-cell hard break (`<br>`) touching the caret on the
    /// `edge` side — the byte range to delete whole. A table row is one source
    /// line, so its break is spelled `<br>` yet drawn as a single newline glyph
    /// (see `wysiwyg`); a delete over it must take every byte, or a one-byte
    /// step strands a broken `<br` in the cell. `Backward` matches a break ending
    /// at the caret (Backspace), `Forward` one starting at it (Delete). `None`
    /// when no such break is adjacent. Only the in-cell break is spelled `<br>`
    /// (an ordinary hard break is `  \n`), so the leading `<` alone tells them
    /// apart — no ancestor walk needed. Rich view only; source view shows the
    /// literal tag and deletes it a byte at a time.
    fn cell_break_at(&mut self, edge: BreakEdge) -> Option<(usize, usize)> {
        let caret = self.caret;
        let nodes = self.nodes();
        let src = self.source.as_bytes();
        nodes
            .iter()
            .find(|n| {
                n.kind == Kind::HardBreak
                    && n.span.start < n.span.end
                    && src.get(n.span.start) == Some(&b'<')
                    && match edge {
                        BreakEdge::Backward => n.span.end == caret,
                        BreakEdge::Forward => n.span.start == caret,
                    }
            })
            .map(|n| (n.span.start, n.span.end))
    }

    /// Whether the source byte at `off` is a backslash twig consumed as an escape
    /// (hidden in the rich view), as against a literal backslash (drawn). A
    /// backslash escapes exactly an ASCII-punctuation character (the CommonMark /
    /// Djot rule twig follows), so `\` + punctuation is the whole test — no AST
    /// round-trip needed.
    fn is_hidden_escape(&self, off: usize) -> bool {
        let b = self.source.as_bytes();
        b.get(off) == Some(&b'\\') && b.get(off + 1).is_some_and(u8::is_ascii_punctuation)
    }

    /// Backspace's list behaviour: when the caret sits exactly at the start of a
    /// list item's content (right after its marker), outdent the item if it's
    /// nested, else strip the marker so it becomes a paragraph. Returns whether
    /// it acted — `false` leaves Backspace its ordinary character delete.
    fn backspace_list_start(&mut self) -> bool {
        let Some(marker) = self.list_marker_on_line(self.caret) else {
            return false;
        };
        // Only right after the marker. That the line opens a real item is
        // already settled: `list_marker_on_line` answers from the tree.
        if self.caret != marker.content_start() {
            return false;
        }
        if self.item_is_nested(marker.marker_start) {
            // Nested: give back one level, keeping the marker and carrying the
            // caret with it.
            self.outdent();
        } else {
            // Top level: drop the marker, leaving a paragraph, then renumber the
            // siblings the removed item was counted among. Only the marker goes —
            // a quote prefix in front of it still has a quote to hold up.
            self.splice(marker.marker_start, self.caret, "", EditKind::Other);
            self.renumber_here();
        }
        true
    }

    /// Backspace's heading behaviour: with the caret exactly at the start of an
    /// ATX heading's content — right after the `#` marker the rich view hides —
    /// strip the marker so the line becomes a paragraph. The peer of
    /// [`backspace_list_start`](Self::backspace_list_start)'s ladder, and the same
    /// reasoning: hidden block markup is structure, so the keystroke over it is
    /// structural.
    ///
    /// Without this the ordinary delete takes the space out of `# Title` and
    /// leaves `#Title`, which is no longer a heading at all — the hash the view
    /// had been hiding surfaces as literal text the user has to delete a second
    /// time, having never typed it. A closing sequence (`# Title #`, hidden at the
    /// other end) goes with the marker for the same reason.
    ///
    /// Returns whether it acted; `false` leaves Backspace its character delete.
    fn backspace_heading_start(&mut self) -> bool {
        let caret = self.caret;
        // The heading whose content opens exactly at the caret. A bare `#` has no
        // content span at all — its content starts (and ends) where the line does.
        let Some((span, content_end, marker)) = self.nodes().iter().find_map(|n| {
            let (start, end) = match &n.content_span {
                Some(c) => (c.start, c.end),
                None => (n.span.end, n.span.end),
            };
            (n.kind == Kind::Heading && start == caret)
                .then(|| (n.span.clone(), end, n.marker_span.clone()))
        }) else {
            return false;
        };
        // twig reports the marker's own extent, so there is nothing to walk back
        // over and no `#` in this file. A setext heading has no marker — its
        // content opens the line — so it falls through to the ordinary delete,
        // as does anything else sitting at a content start.
        // `m.end == caret` is what excludes a setext heading, whose marker is the
        // underline *after* the content rather than a prefix before it.
        let Some(marker) = marker.filter(|m| m.end == caret) else {
            return false;
        };
        let start = marker.start;
        // A closing `#` sequence is hidden too, so it can't be left behind. Only
        // when the tail really is one: trailing spaces alone are nothing to strip.
        let tail = &self.source[content_end..span.end];
        if tail.contains('#') && tail.chars().all(|c| c == '#' || c.is_whitespace()) {
            let kept = self.source[caret..content_end].to_string();
            self.splice(start, span.end, &kept, EditKind::Other);
            // The splice leaves the caret past the text it re-wrote; the caret
            // belongs where the content now starts, which is where it already was.
            self.caret = start;
            self.record_caret();
        } else {
            self.splice(start, caret, "", EditKind::Other);
        }
        true
    }

    pub fn delete_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
        } else if self.caret < self.source.len() {
            // The mirror of Backspace's: forward-delete in front of a picture
            // would eat the `!` off its markup and leave a link where a photo was.
            if self.view != View::Source && self.delete_around_block_atom(true) {
                return;
            }
            // And of Backspace's cell wall: Delete at a cell's end would take
            // the padding and the `|` after it.
            if self.view != View::Source && self.at_cell_wall(BreakEdge::Forward) {
                return;
            }
            // And of Backspace's join: at the end of a block's content, Delete
            // joins the next block into this one.
            if self.view != View::Source && self.delete_forward_joins_block() {
                return;
            }
            // Delete forward over an in-cell `<br>` takes the whole tag, the mirror
            // of Backspace's swallow (see `cell_break_at`) — else a byte-step
            // strands a broken `<br` in the cell.
            if self.view != View::Source
                && let Some((start, end)) = self.cell_break_at(BreakEdge::Forward)
            {
                self.splice(start, end, "", EditKind::Delete);
                return;
            }
            // The mirror of Backspace's two steps: from in front of a run's
            // opening `**` step into it, onto the first letter of its text, and
            // at the end of a run's text step out past its closing `**` to the
            // character beyond. Either way Delete takes the character it looks
            // like it is pointing at, and never a delimiter drawn as nothing.
            // The caret then settles back inside the run it was standing in —
            // see `settle_inside_close_delims`.
            let from = if self.view == View::Source {
                self.caret
            } else {
                let inside = self.step_inside_open_delims(self.caret);
                // The mirror of Backspace's empty-span rule: an attributed
                // span with no text goes whole, with the character after it.
                if let Some(span) = self.run_span_of_content(inside..inside) {
                    let to = if self.source[span.end..].starts_with('\n') {
                        span.end
                    } else {
                        next_boundary(&self.source, span.end)
                    };
                    self.splice(span.start, to, "", EditKind::Delete);
                    return;
                }
                self.skip_trailing_close_delims(inside)
            };
            let next = next_boundary(&self.source, from);
            // And of its emptying rules: the span goes with its last letter,
            // and so does the block's div or `{…}` line.
            if self.view != View::Source
                && let Some(span) = self.run_span_of_content(from..next)
            {
                self.splice(span.start, span.end, "", EditKind::Delete);
                return;
            }
            if self.view != View::Source
                && let Some(block) = self.attributed_block_of_content(from..next)
            {
                self.splice(block.start, block.end, "", EditKind::Delete);
                return;
            }
            if from < next {
                self.splice(from, next, "", EditKind::Delete);
            }
        }
    }

    /// Delete from the caret back to the start of the previous word (⌥⌫ /
    /// Ctrl+⌫). Deletes the selection instead when one is active.
    pub fn delete_word_back(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
        } else {
            // A word back from just past a picture is a word *of its markup*, and
            // a word back from in front of one runs through the paragraph break
            // into the prose above — dissolving the picture either way. See
            // `delete_around_block_atom`.
            if self.view != View::Source && self.delete_around_block_atom(false) {
                return;
            }
            let start = self.word_left_from(self.caret).max(self.caret_floor());
            if start < self.caret {
                let (s, e) = self.widen_over_emptied_inlines(start, self.caret);
                self.splice(s, e, "", EditKind::Delete);
            }
        }
    }

    /// Delete from the caret forward to the end of the next word (⌥⌦ /
    /// Ctrl+Del). Deletes the selection instead when one is active.
    pub fn delete_word_forward(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
        } else {
            // The mirror: a word forward from in front of a picture is its markup.
            if self.view != View::Source && self.delete_around_block_atom(true) {
                return;
            }
            let end = self.word_right_from(self.caret);
            if end > self.caret {
                let (s, e) = self.widen_over_emptied_inlines(self.caret, end);
                self.splice(s, e, "", EditKind::Delete);
            }
        }
    }

    /// Delete from the caret back to the start of its line (⌘⌫). Deletes the
    /// selection instead when one is active, as every other delete here does.
    ///
    /// The line is the view's own — the one Home and End work on, so in WYSIWYG
    /// a soft-wrapped row is a line. It is not Home's *target*, though: Home
    /// stops at the first character and this takes the indentation with it, the
    /// way Cocoa's `deleteToBeginningOfLine:` does. Stopping at the text would
    /// leave an indent behind that nothing can then ask to delete, where a caret
    /// left at column 0 is one press of Home away from either.
    pub fn delete_to_line_start(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
            return;
        }
        // Never back across the floor: hidden frontmatter isn't on this line, or
        // on any line the WYSIWYG caret can see.
        let (start, _) = self.line_span();
        let start = start.max(self.caret_floor());
        if start < self.caret {
            let (s, e) = self.widen_over_emptied_inlines(start, self.caret);
            self.splice(s, e, "", EditKind::Delete);
        }
    }

    /// Kill from the caret to the end of its line (^K). Deletes the selection
    /// instead when one is active.
    ///
    /// At the end of the line it does nothing, rather than pulling the line
    /// below up into this one. Joining has no meaning to give it in both views
    /// at once: a WYSIWYG line ends at a soft wrap as often as at a newline, and
    /// there is nothing there to delete, while the newline a *source* line ends
    /// with is only half of the blank line that separates two paragraphs —
    /// deleting one leaves a soft break, which is not the join it looks like.
    /// The views agreeing is worth more than emacs' second press, and Delete is
    /// already the key that joins.
    pub fn delete_to_line_end(&mut self) {
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
            return;
        }
        let (_, end) = self.line_span();
        if end > self.caret {
            let (s, e) = self.widen_over_emptied_inlines(self.caret, end);
            self.splice(s, e, "", EditKind::Delete);
        }
    }

    /// Grow a WYSIWYG word-delete to swallow any inline node it empties.
    ///
    /// A glyph-space range covers what the user can see, which for `**bold**` is
    /// the word and never the delimiters around it — so deleting the word on its
    /// own leaves `a **** c`, markup wrapped around nothing. They asked for the
    /// word, and the styling was the word's; the two go together. Only the
    /// node's delimiters are taken, and those are hidden here anyway, so nothing
    /// visible outside the range is lost.
    ///
    /// Repeated to a fixed point: emptying `***bold***` empties the emph inside
    /// the strong, and only then is the strong empty too.
    fn widen_over_emptied_inlines(&mut self, start: usize, end: usize) -> (usize, usize) {
        if self.view == View::Source {
            return (start, end);
        }
        let nodes = self.nodes();
        let (mut s, mut e) = (start, end);
        loop {
            let mut grew = false;
            for n in nodes.iter().filter(|n| wysiwyg::is_inline(n)) {
                let Some(text) = inline_content_span(n, &self.source) else {
                    continue;
                };
                // Some of its text survives, so the node still has a job.
                if text.start < s || text.end > e {
                    continue;
                }
                if n.span.start < s || n.span.end > e {
                    s = s.min(n.span.start);
                    e = e.max(n.span.end);
                    grew = true;
                }
            }
            if !grew {
                return (s, e);
            }
        }
    }
}
