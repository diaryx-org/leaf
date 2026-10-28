//! Moving the caret and selection: by character, word, row, line, and cell,
//! and placing it with a click.

use super::*;

impl Doc {
    // ── caret movement ─────────────────────────────────────────────────────────
    // `extend` grows the selection (Shift+motion): it pins the anchor on the
    // first extended step and moves only the caret; an un-extended motion drops
    // the selection.

    /// Place the caret at byte `offset` (clamped to a char boundary), extending
    /// the selection when `extend` is set. The public form of `move_to`, for a
    /// frontend that hit-tests pixels straight to a source offset.
    pub fn place_caret(&mut self, offset: usize, extend: bool) {
        self.goal_col = None;
        let before = self.caret;
        // A pixel hit-test can land between the visible caret stops — in the
        // blank gap a paragraph break is drawn with, or inside a hidden delimiter.
        // Snap to the nearest real stop so the caret can't come to rest where it
        // would draw in one place and type in another. The `(row, col)` click
        // path (`click`) already snaps this way through `offset_of_pos`; the
        // source view reaches every byte, so it snaps to nothing.
        let target = match self.view {
            View::Wysiwyg => self.snap_to_stop_on_its_line(offset.min(self.source.len())),
            // The source view reaches every byte, so there is no stop to snap
            // to — but "every byte" still means every *character* boundary. A
            // caret resting inside a multi-byte character draws nowhere real
            // and panics the next time anything slices there.
            View::Source => self.char_boundary_at_or_before(offset),
        };
        self.move_to(target, extend);
        self.clamp_caret();
        self.debug_assert_on_a_stop(before);
    }

    /// The caret stop `off` snaps to in WYSIWYG — the nearest one, unless `off`
    /// is in hidden markup that *opens* a line (a heading's `## `, a list's
    /// `- `), in which case the first stop on that line.
    ///
    /// Nearest alone is wrong there because the stop behind such markup is on
    /// an earlier line: from the `#` of `## End`, the end of the paragraph
    /// above is two bytes back and the `E` three forward. A block's first byte
    /// is exactly what [`locate`](Self::locate) hands a frontend to land on, so
    /// following `#end` put the reader before the heading they were sent to.
    /// A blank line opens nothing and has no stop of its own, so a click in
    /// the gap between blocks still goes to the nearer side.
    fn snap_to_stop_on_its_line(&self, off: usize) -> usize {
        let nearest = self.vmap.snap_to_stop(off);
        if nearest >= off {
            return nearest;
        }
        let bytes = self.source.as_bytes();
        let line_start = bytes[..off]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        if nearest >= line_start {
            return nearest;
        }
        let line_end = bytes[off..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| off + i);
        match self.vmap.stop_at_or_after(off) {
            Some(stop) if stop <= line_end => stop,
            _ => nearest,
        }
    }

    /// Select the whole document (⌘A / Ctrl+A) — everything reachable in the
    /// active view, so in WYSIWYG it starts below hidden frontmatter (copy won't
    /// grab the metadata) while the source view still selects the literal whole.
    pub fn select_all(&mut self) {
        self.anchor = Some(self.caret_floor());
        self.caret = self.source.len();
        self.goal_col = None;
        self.last_edit_kind = None;
        self.status = None;
    }

    /// Select the word (or whitespace / punctuation run) at `offset` — the
    /// double-click gesture. Anchors on the run's start with the caret at its
    /// end so a following Shift-motion extends from the far edge.
    pub fn select_word_at(&mut self, offset: usize) {
        let src = &self.source;
        let mut off = offset.min(src.len());
        while off > 0 && !src.is_char_boundary(off) {
            off -= 1;
        }
        // At a line's end the character under the press is the line break:
        // the press means the last word on the line, not the break; on an
        // empty line there is nothing to select.
        if src[off..]
            .chars()
            .next()
            .is_none_or(|c| c == '\n' || c == '\r')
        {
            match src[..off].chars().next_back() {
                Some(c) if c != '\n' && c != '\r' => off -= c.len_utf8(),
                _ => {
                    self.place_caret(off, false);
                    return;
                }
            }
        }
        let (s, e) = word_range_at(src, off);
        self.anchor = Some(s);
        self.caret = e;
        self.goal_col = None;
        self.last_edit_kind = None;
        self.status = None;
        self.clamp_caret();
    }

    /// The word at `offset`, as source bytes, without touching the selection —
    /// what a host asks before it does something *with* a word rather than to
    /// the selection: the Mac's Look Up under a force click, a case change at
    /// a bare caret.
    ///
    /// The word the character at `offset` is in, or failing that the one the
    /// character before it ends — a point on the trailing half of a word's last
    /// letter hit-tests to the boundary after it, and still means that word.
    /// `None` between words, where a double-click would select the space; a
    /// host looking a word up has nothing to look up there.
    ///
    /// In WYSIWYG the word is found among the characters drawn, as word motion
    /// finds it: `_` is a word character, so a raw walk of `_important_` would
    /// take the hidden delimiters with it, and a host would look up, case-change
    /// or highlight markup the reader cannot see.
    pub fn word_at(&self, offset: usize) -> Option<Range<usize>> {
        if self.view == View::Wysiwyg && self.vmap.num_rows() > 0 {
            return self.glyph_word_at(offset);
        }
        let s = &self.source;
        let mut off = offset.min(s.len());
        while off > 0 && !s.is_char_boundary(off) {
            off -= 1;
        }
        let at = s[off..]
            .chars()
            .next()
            .filter(|&c| classify(c) == Class::Word);
        let before = s[..off]
            .chars()
            .next_back()
            .filter(|&c| classify(c) == Class::Word);
        let probe = match (at, before) {
            (Some(_), _) => off,
            (None, Some(c)) => off - c.len_utf8(),
            (None, None) => return None,
        };
        let (start, end) = word_range_at(s, probe);
        Some(start..end)
    }

    /// [`word_at`](Self::word_at) in glyph space: the run of word-class stops
    /// holding the glyph drawn at `offset` (where a caret there draws), or the
    /// glyph before it. Ends after the last word glyph's own character, so a
    /// closing delimiter hidden behind it is left out.
    fn glyph_word_at(&self, offset: usize) -> Option<Range<usize>> {
        let word = |off: usize| self.class_at(off) == Class::Word;
        let at = self.vmap.stop_at_or_after(offset);
        let before = at.map_or_else(
            || self.vmap.stop_at_or_before(offset),
            |a| self.vmap.stop_before(a),
        );
        let probe = match (at, before) {
            (Some(a), _) if word(a) => a,
            (_, Some(b)) if word(b) => b,
            _ => return None,
        };
        let mut start = probe;
        while let Some(prev) = self.vmap.stop_before(start).filter(|&p| word(p)) {
            start = prev;
        }
        let mut last = probe;
        while let Some(next) = self.vmap.stop_after(last).filter(|&n| word(n)) {
            last = next;
        }
        let end = last + self.source[last..].chars().next().map_or(0, char::len_utf8);
        Some(start..end)
    }

    /// Select the whole enclosing text block (paragraph, heading, list item's
    /// text…) at `offset` — the triple-click gesture. Reads the range straight
    /// from the AST (twig's `content_span`), so it selects the entire *logical*
    /// paragraph even when that paragraph soft-wraps across several visual rows —
    /// where a visual-row-based select breaks down, because one source offset at
    /// a wrap boundary belongs to two rows at once.
    pub fn select_block_at(&mut self, offset: usize) {
        let off = offset.min(self.source.len());
        let range = self
            .editor
            .ancestors_at(off)
            .ok()
            .and_then(|chain| {
                // Ancestors run root → deepest; the deepest node that is neither
                // an inline span nor a multi-block container is the text block
                // the caret sits in (a paragraph, a heading, a code block…).
                chain
                    .into_iter()
                    .rev()
                    .find(|m| !wysiwyg::is_inline_kind(&m.kind) && !is_block_container(&m.kind))
                    .map(|m| m.content_span.unwrap_or(m.span))
            })
            .unwrap_or_else(|| source_line_range(&self.source, off));
        self.anchor = Some(range.start.min(self.source.len()));
        self.caret = range.end.min(self.source.len());
        self.goal_col = None;
        self.last_edit_kind = None;
        self.status = None;
        self.clamp_caret();
    }

    /// Select the exact source range `[start, end)` — anchor at `start`, caret
    /// at `end` — without snapping either end to a visible caret stop.
    ///
    /// The one caret verb that takes a range it was *handed* rather than one it
    /// worked out, for a host that already knows the bytes it means: a search
    /// hit, an annotation's footprint, a quote re-anchored through
    /// [`Doc::selection_quote`]. [`place_caret`](Self::place_caret) is the
    /// wrong tool for that, and not by a little — it snaps to the nearest
    /// *visible* stop, and where a range butts up against a hidden delimiter
    /// the nearest stop is the one before it, so selecting the "needle" of
    /// `**needle**` comes back with "needl" and an edit against it strands the
    /// "e".
    ///
    /// What `place_caret` does that is bookkeeping rather than snapping still
    /// happens here, because a host handing in a range is not asking to opt out
    /// of the invariants:
    ///
    /// - both ends are clamped into the document and up to
    ///   [`caret_floor`](Self::caret_floor) — in WYSIWYG the leading
    ///   frontmatter is hidden, and a caret parked in it draws nowhere and
    ///   types into the metadata;
    /// - both land on character boundaries, so nothing slices a `é` in half;
    /// - the sticky vertical goal column is dropped, and any armed inline mark
    ///   disarmed, since a range from outside inherits neither.
    ///
    /// An empty range is a caret rather than a selection —
    /// [`selection`](Self::selection) reports `None` for it, as it does for any
    /// anchor that has met the caret.
    pub fn select_range(&mut self, start: usize, end: usize) {
        let floor = self.caret_floor();
        let anchor = self.char_boundary_at_or_before(start.clamp(floor, self.source.len()));
        let caret = self.char_boundary_at_or_before(end.clamp(floor, self.source.len()));
        self.anchor = Some(anchor);
        self.caret = caret;
        self.goal_col = None;
        self.status = None;
        self.last_edit_kind = None;
        self.clear_pending();
    }

    /// `offset` itself if it is a character boundary, else the boundary before
    /// it. An offset that isn't one draws nowhere real and panics the next time
    /// anything slices there.
    fn char_boundary_at_or_before(&self, offset: usize) -> usize {
        let mut o = offset.min(self.source.len());
        while o > 0 && !self.source.is_char_boundary(o) {
            o -= 1;
        }
        o
    }

    /// The lowest source offset the caret may occupy in the active view. In
    /// WYSIWYG, leading frontmatter is hidden and unreachable, so the floor is
    /// the first rendered offset; the source view reaches everything, so it's 0.
    pub(super) fn caret_floor(&self) -> usize {
        match self.view {
            View::Wysiwyg => self.vmap.content_start.min(self.source.len()),
            View::Source => 0,
        }
    }

    /// Land in a table cell with its whole content selected — the anchor at the
    /// cell's start, the caret at its end — so a Tab/Return hop into a cell reads
    /// like tabbing into a form field: the text comes up selected, so typing
    /// replaces it and an arrow collapses to an edge. An empty cell (`start ==
    /// end`) collapses to a plain caret home (an empty selection is no selection).
    pub(super) fn select_cell(&mut self, start: usize, end: usize) {
        self.select_range(start, end);
    }

    fn move_to(&mut self, offset: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
        self.caret = offset.min(self.source.len()).max(self.caret_floor());
        self.status = None;
        // A caret move ends the current typing/deletion run, so the next edit
        // starts a fresh undo group rather than coalescing across the gap.
        self.last_edit_kind = None;
        // Moving away disarms any sticky mark — "start bold" applies only where
        // it was asked for, not wherever the caret next lands.
        self.clear_pending();
    }

    // In the source view, motion walks source bytes / source lines. In the
    // WYSIWYG view it walks the rendered glyph grid (the visual map), which is
    // what steps the caret cleanly over hidden delimiters.

    pub fn move_left(&mut self, extend: bool) {
        self.goal_col = None;
        if !extend && let Some((s, _e)) = self.selection() {
            self.move_to(s, false);
            return;
        }
        let target = match self.view {
            View::Source => {
                if self.caret > 0 {
                    prev_boundary(&self.source, self.caret)
                } else {
                    0
                }
            }
            // Walks caret *stops*, not columns: decoration (a table border, a
            // cell's padding) is stepped over in one press, and a hidden
            // delimiter never holds the caret up — though the end of a mark's
            // content is a stop of its own (`VisualMap::mark_ends`), so
            // leaving `**bold**` from past its `**` is a press onto the end of
            // the bold and another onto the `d`.
            View::Wysiwyg => self
                .vmap
                .caret_stop_before(self.caret)
                .unwrap_or(self.caret),
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    pub fn move_right(&mut self, extend: bool) {
        self.goal_col = None;
        if !extend && let Some((_s, e)) = self.selection() {
            self.move_to(e, false);
            return;
        }
        let target = match self.view {
            View::Source => {
                if self.caret < self.source.len() {
                    next_boundary(&self.source, self.caret)
                } else {
                    self.caret
                }
            }
            View::Wysiwyg => self.vmap.caret_stop_after(self.caret).unwrap_or(self.caret),
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// Move to the start of the previous word (⌥← / Ctrl+←).
    pub fn move_word_left(&mut self, extend: bool) {
        self.goal_col = None;
        let before = self.caret;
        let target = self.word_left_from(self.caret);
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// Move to the end of the next word (⌥→ / Ctrl+→).
    pub fn move_word_right(&mut self, extend: bool) {
        self.goal_col = None;
        let before = self.caret;
        let target = self.word_right_from(self.caret);
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    // Word boundaries are found in the space the *view* is in. The source view
    // walks the source, because there the source is what's rendered. WYSIWYG
    // walks the rendered text instead: `**` is invisible to the user, so it has
    // to be invisible to word motion too — a caret parked inside one draws in
    // the column after `bold` and types two bytes earlier, and a word-delete
    // that stops there shreds the markup into `a ** c`.

    /// The word boundary to the left of `off` in the active view's space.
    pub(super) fn word_left_from(&self, off: usize) -> usize {
        match self.view {
            View::Source => prev_word(&self.source, off),
            View::Wysiwyg => self.glyph_word_left(off),
        }
    }

    /// The word boundary to the right of `off` in the active view's space.
    pub(super) fn word_right_from(&self, off: usize) -> usize {
        match self.view {
            View::Source => next_word(&self.source, off),
            View::Wysiwyg => self.glyph_word_right(off),
        }
    }

    /// The character class of the glyph drawn at stop `off`.
    ///
    /// Read from the source, because a stop points at the source byte its glyph
    /// came from — the source *is* where the rendered character is written. What
    /// makes the walk glyph space rather than source space is that it only ever
    /// visits stops, and the hidden bytes between them have none.
    fn class_at(&self, off: usize) -> Class {
        self.source
            .get(off..)
            .and_then(|s| s.chars().next())
            .map_or(Class::Space, classify)
    }

    /// [`next_word`] in glyph space: skip any leading separators, then consume
    /// the following word run, with the stop table standing in for the source's
    /// characters.
    fn glyph_word_right(&self, from: usize) -> usize {
        let Some(mut off) = self.vmap.stop_at_or_after(from) else {
            return from;
        };
        let mut in_word = false;
        loop {
            match self.class_at(off) {
                Class::Word => in_word = true,
                _ if in_word => return off,
                _ => {}
            }
            match self.vmap.stop_after(off) {
                Some(next) => off = next,
                None => return off,
            }
        }
    }

    /// [`prev_word`] in glyph space: skip separators walking left, then consume
    /// the preceding word run.
    fn glyph_word_left(&self, from: usize) -> usize {
        let Some(mut off) = self.vmap.stop_at_or_before(from) else {
            return from;
        };
        let mut in_word = false;
        while let Some(prev) = self.vmap.stop_before(off) {
            match self.class_at(prev) {
                Class::Word => in_word = true,
                _ if in_word => return off,
                _ => {}
            }
            off = prev;
        }
        off
    }

    /// After a motion that walks the visual map, the caret must be *on* the map.
    /// A stop is the only offset where the caret draws and edits in the same
    /// place, and it's the invariant both a caret parked inside an emoji and one
    /// parked inside a `**` were quietly breaking.
    ///
    /// Only when the caret actually moved: a walk with nowhere to go leaves it
    /// where it was, which is wherever the floor or a frontend put it rather
    /// than somewhere this motion chose.
    fn debug_assert_on_a_stop(&self, before: usize) {
        debug_assert!(
            self.view != View::Wysiwyg
                || self.vmap.num_rows() == 0
                || self.caret == before
                || self.vmap.is_stop(self.caret),
            "motion left the caret at {}, which is not a caret stop: it would draw in \
             one place and type in another",
            self.caret
        );
    }

    // Up and Down run off the ends of the document rather than stopping dead at
    // them: Up from the first row lands at the document's start, Down from the
    // last at its end. That's Cocoa's rule (`moveUp:`/`moveDown:` past the edge
    // are `moveToBeginningOfDocument:`/`moveToEndOfDocument:`), and holding ↓
    // reaching the end of the text is what a reader means by it.
    //
    // The views used to disagree here by accident rather than by decision: the
    // source view fell into the edge behaviour through `row_col_to_offset`
    // clamping an out-of-range row to the end of the string, while WYSIWYG had
    // no row below to walk to and did nothing at all. They share the rule now,
    // each in its own space — the source view reaches every byte, WYSIWYG only
    // the offsets it draws.

    pub fn move_up(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        let goal = self.goal_col.unwrap_or(col);
        let target = match self.view {
            View::Source => match row.checked_sub(1) {
                Some(r) => row_col_to_offset(&self.source, r, goal),
                None => self.reachable_start(),
            },
            // A table's border rules are drawn but hold no caret, so Up steps
            // over them to the row that does.
            View::Wysiwyg => match self.vmap.navigable_above(row) {
                Some(r) => self.row_target(r, goal),
                None => self.reachable_start(),
            },
        };
        self.step_vertical(target, goal, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        let (row, col) = self.caret_pos();
        let goal = self.goal_col.unwrap_or(col);
        let target = match self.view {
            View::Source => match self.source_row_below(row) {
                Some(r) => row_col_to_offset(&self.source, r, goal),
                None => self.reachable_end(),
            },
            View::Wysiwyg => match self.vmap.navigable_below(row) {
                Some(r) => self.row_target(r, goal),
                None => self.reachable_end(),
            },
        };
        self.step_vertical(target, goal, extend);
    }

    /// Land a vertical motion at `target`, latching the `goal` column it aimed
    /// with so the rest of the run keeps aiming there.
    ///
    /// A motion with nowhere to go changes *nothing*, the goal column included:
    /// the latch used to run before the early return at the top of the document,
    /// so an Up that did nothing still armed a column, and the next Down aimed
    /// at one the caret had never been in.
    fn step_vertical(&mut self, target: usize, goal: usize, extend: bool) {
        let before = self.caret;
        if target == before {
            return;
        }
        self.goal_col = Some(goal);
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// The source line below `row`, or `None` when `row` is the last one. Lines
    /// are counted by newline, so a trailing one leaves a real, empty last line
    /// for the caret to sit on — the document ends below it, not on it.
    fn source_row_below(&self, row: usize) -> Option<usize> {
        let last = self.source.bytes().filter(|&b| b == b'\n').count();
        (row < last).then_some(row + 1)
    }

    /// Where a vertical motion aiming at the `goal` column lands on visual row
    /// `r`: the column clamped to the row, mapped to its offset, then held
    /// inside the row's own [bounds](Self::row_bounds) — a wrapped row's last
    /// column belongs to the row below, and a gutter's column 0 points at the
    /// block rather than at this row.
    fn row_target(&self, r: usize, goal: usize) -> usize {
        let (start, end) = self.row_bounds(r);
        self.vmap
            .offset_of_pos(r, goal.min(self.vmap.row_width(r)))
            .clamp(start, end)
    }

    /// The first and last offsets the caret can reach in the active view.
    ///
    /// Not the same span in both: the source view shows every byte, so it can
    /// reach every byte. WYSIWYG reaches only what it draws — hidden frontmatter
    /// sits below the first stop, and a document's trailing newline is drawn
    /// nowhere and so sits past the last.
    fn reachable_start(&self) -> usize {
        match self.view {
            View::Source => 0,
            View::Wysiwyg => self.vmap.stop_at_or_after(0).unwrap_or(self.caret),
        }
    }

    fn reachable_end(&self) -> usize {
        match self.view {
            View::Source => self.source.len(),
            View::Wysiwyg => self
                .vmap
                .stop_at_or_before(self.source.len())
                .unwrap_or(self.caret),
        }
    }

    /// The `[start, end]` offsets visual row `r` *draws* — everything on it,
    /// including the space a soft wrap ate off its end, which is drawn on this
    /// row however much the offset past it belongs to the next one.
    fn row_span(&self, r: usize) -> (usize, usize) {
        let start = self
            .vmap
            .row_start(r)
            .unwrap_or_else(|| self.vmap.offset_of_pos(r, 0));
        let end = self.vmap.offset_of_pos(r, self.vmap.row_width(r));
        (start.min(end), end)
    }

    /// [`row_span`](Self::row_span) narrowed to where the caret can stand: a
    /// soft wrap's shared offset opens the row below (see `pos_of_offset`), so
    /// this row's last position is the one before it — the offset before the
    /// space the wrap ate, where the caret draws just past the row's last word
    /// and types there too.
    ///
    /// Aiming at the shared offset instead is what stalled End: it is the row's
    /// last *column*, so End pressed on the row reached it and then read back as
    /// the row below's start, where a second press ran on to that row's end and
    /// the next to the one after — End walking down the paragraph a row a press.
    fn row_bounds(&self, r: usize) -> (usize, usize) {
        let (start, end) = self.row_span(r);
        let wraps = self
            .vmap
            .navigable_below(r)
            .and_then(|b| self.vmap.row_start(b))
            .is_some_and(|off| off == end);
        match wraps {
            true => (start, self.vmap.stop_before(end).unwrap_or(end).max(start)),
            false => (start, end),
        }
    }

    /// The `[start, end]` of the line Home and End aim at: the visual row in
    /// WYSIWYG, the logical line in the source view. Both ends are caret stops.
    ///
    /// A soft-wrapped row is a line here, because it is one to the eye and the
    /// eye is what these keys are aimed by — a reader pressing End means the end
    /// of the line they can see. (`select_block_at` wants the opposite and reads
    /// the AST for it: a triple-click grabs the whole paragraph, however many
    /// rows it folds into.)
    fn line_bounds(&self) -> (usize, usize) {
        let (row, _) = self.caret_pos();
        match self.view {
            View::Source => {
                let start = line_start(&self.source, row);
                (start, line_end_from(&self.source, start))
            }
            View::Wysiwyg => self.row_bounds(row),
        }
    }

    /// The same line as [`line_bounds`](Self::line_bounds), as far as it is
    /// *drawn* — what a kill takes.
    ///
    /// The two part only at a soft wrap, over the space the wrap ate: the caret
    /// can't stand after it (that offset opens the row below, and End stopping
    /// there would walk), but it is on this row, and a kill that spared it would
    /// leave a double space behind where the row's text had been. Deleting it
    /// joins nothing — a wrap is drawn, not written.
    pub(super) fn line_span(&self) -> (usize, usize) {
        let (row, _) = self.caret_pos();
        match self.view {
            View::Source => self.line_bounds(),
            View::Wysiwyg => self.row_span(row),
        }
    }

    /// The first offset in `[start, end]` holding something other than
    /// whitespace, or `end` when the line holds nothing else — where Home aims.
    ///
    /// Walks the space the view is in, as word motion does: WYSIWYG steps stops,
    /// so a hidden delimiter is never taken for the line's first character (nor
    /// landed on), and the source view steps the source it is showing.
    fn first_non_space(&self, start: usize, end: usize) -> usize {
        let mut off = start;
        while off < end {
            if self.class_at(off) != Class::Space {
                return off;
            }
            off = match self.view {
                View::Source => next_boundary(&self.source, off),
                View::Wysiwyg => match self.vmap.stop_after(off) {
                    Some(next) => next,
                    None => return end,
                },
            };
        }
        end
    }

    /// Home: to the first character on the line, or to column 0 when the caret
    /// is already on it — the two-press toggle every editor spells this way.
    /// The indentation is somewhere the caret has to be able to reach and almost
    /// never where a reader is headed, so it costs the second press.
    pub fn move_home(&mut self, extend: bool) {
        self.goal_col = None;
        let (start, end) = self.line_bounds();
        let text = self.first_non_space(start, end);
        let target = if self.caret == text { start } else { text };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// End: to the end of the line.
    pub fn move_end(&mut self, extend: bool) {
        self.goal_col = None;
        let (_, end) = self.line_bounds();
        let before = self.caret;
        self.move_to(end, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// Hop to the next (Tab) or previous (Shift+Tab) table cell, landing with the
    /// cell's whole content selected (see [`Self::select_cell`]). Returns `false`
    /// when the caret isn't in a table, or is already in the last/first cell — the
    /// frontend then does whatever Tab normally does (indent), so Tab keeps its
    /// meaning everywhere else.
    pub fn cell_hop(&mut self, forward: bool) -> bool {
        let Some((grid, r, c)) = self.table_grid_at(self.caret) else {
            return false;
        };
        // Flatten to document (row-major) order and step one cell either way.
        let i: usize = grid[..r].iter().map(Vec::len).sum::<usize>() + c;
        let flat: Vec<(usize, usize)> = grid.into_iter().flatten().collect();
        let next = if forward {
            i.checked_add(1)
        } else {
            i.checked_sub(1)
        };
        let Some(&(start, end)) = next.and_then(|j| flat.get(j)) else {
            return false; // at the table's edge; leave Tab to the frontend
        };
        self.select_cell(start, end);
        true
    }

    /// Move the caret to the cell directly above (`down == false`) or below in
    /// the same column, landing with the cell's whole content selected (see
    /// [`Self::select_cell`]). Returns `false` at the grid's top/bottom edge (or
    /// when the caret isn't in a table), so the frontend can fall through — the
    /// vertical counterpart of [`Self::cell_hop`].
    ///
    /// A ragged row that is short a column clamps to its last cell, so Down never
    /// falls out of the table over a gap the row above happened to have.
    pub fn cell_move_vertical(&mut self, down: bool) -> bool {
        let Some((grid, r, c)) = self.table_grid_at(self.caret) else {
            return false;
        };
        let target = match down {
            true => r + 1,
            false if r == 0 => return false,
            false => r - 1,
        };
        let Some(row) = grid.get(target) else {
            return false;
        };
        let Some(&(start, end)) = row.get(c).or_else(|| row.last()) else {
            return false;
        };
        self.select_cell(start, end);
        true
    }

    /// The table containing `off` as a row-major grid of `(start, end)` cell
    /// caret homes, plus the `(row, col)` the caret sits in — `None` when `off`
    /// isn't in a table. Read straight off the visual map's laid-out grid, so
    /// every cell (an empty one included, whose derived home twig gives no
    /// `content_span` for) is present and in the order Tab walks them.
    // Grid, row, column — three returns that only ever travel together, and a
    // named type for the pair of them would be read at one call site.
    #[allow(clippy::type_complexity)]
    pub(super) fn table_grid_at(
        &self,
        off: usize,
    ) -> Option<(Vec<Vec<(usize, usize)>>, usize, usize)> {
        for t in &self.vmap.tables {
            let mut pos = None;
            let grid: Vec<Vec<(usize, usize)>> = t
                .grid
                .iter()
                .enumerate()
                .map(|(r, row)| {
                    row.cells
                        .iter()
                        .enumerate()
                        .map(|(c, cell)| {
                            if pos.is_none() && off >= cell.start && off <= cell.end {
                                pos = Some((r, c));
                            }
                            (cell.start, cell.end)
                        })
                        .collect()
                })
                .collect();
            if let Some((r, c)) = pos {
                return Some((grid, r, c));
            }
        }
        None
    }
}

impl Doc {
    /// Rebuild the visual map at the width the last build used. A structural edit
    /// bumps the revision and swaps the source in, but leaves the *map* stale;
    /// when a single gesture edits and then moves over the result (Tab appending
    /// a row, then stepping into it), the move needs the map to already show the
    /// edit rather than waiting for the frontend's next frame.
    pub(super) fn rebuild_map(&mut self) {
        let wrap = self.vmap_key.as_ref().and_then(|(_, w, _)| *w);
        self.build_map(wrap);
    }

    /// Move the caret to the very start of the document (⌘↑ on macOS,
    /// Ctrl+Home on Windows/Linux).
    pub fn move_doc_start(&mut self, extend: bool) {
        self.goal_col = None;
        self.move_to(0, extend);
    }

    /// Move the caret to the very end of the document (⌘↓ on macOS,
    /// Ctrl+End on Windows/Linux).
    pub fn move_doc_end(&mut self, extend: bool) {
        self.goal_col = None;
        // In the rich view the end is the last caret stop, after the last
        // letter: past the file's trailing newline, typing would start a line
        // the view does not draw.
        let end = match self.view {
            View::Wysiwyg => self.vmap.snap_to_stop(self.source.len()),
            View::Source => self.source.len(),
        };
        self.move_to(end, extend);
    }

    /// Point the caret at the body cell `(row, col)` the mouse landed on —
    /// `col` being a cell of the terminal grid, which is what a display column
    /// is. A click on the far cell of a wide character lands at that
    /// character's start; the mapping's own doc-comments carry the rule.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) {
        self.goal_col = None;
        let target = match self.view {
            View::Source => row_col_to_offset(&self.source, row, col),
            View::Wysiwyg => self.vmap.offset_of_pos(row, col),
        };
        let before = self.caret;
        self.move_to(target, extend);
        self.debug_assert_on_a_stop(before);
    }

    /// A click in the blank space under the document's last block.
    ///
    /// Not a click *on* anything, so it lands on nothing in particular: the
    /// caret goes onto an empty paragraph under the last block, wherever the
    /// pointer was horizontally — and if the document does not end with one,
    /// one is opened, which is the only way to get out from under a block Enter
    /// cannot leave. Enter inside a fenced code block is a literal newline (see
    /// [`newline`](Self::newline)), so a document that *ends* in a fence had no
    /// way out at all; and a click under any last block used to land at the
    /// pointer's x on the block's last line, which is what a click on that line
    /// means and not what a click under it does.
    ///
    /// "Ends with an empty paragraph" is two trailing newlines: the first closes
    /// the last line and the second opens the blank line the visual map lays
    /// out as a navigable empty row (see `emit_trailing_blank_lines`). A
    /// document ending inside an *unclosed* fence gets the fence closed first,
    /// since a newline written there would only be another line of code. An
    /// empty document has nothing to be under, and the caret simply goes to its
    /// start.
    ///
    /// In the source view the gesture is the ordinary one: the caret goes to the
    /// end of the source, and nothing is written. A read-only document likewise.
    pub fn click_past_end(&mut self) {
        self.goal_col = None;
        let len = self.source.len();
        if self.view == View::Source || self.read_only || self.source.trim().is_empty() {
            self.move_to(len, false);
            return;
        }
        let mut tail = String::new();
        if let Some(fence) = self.unclosed_fence_at_end() {
            if !self.source.ends_with('\n') {
                tail.push('\n');
            }
            tail.push_str(&fence);
            tail.push('\n');
        }
        let joined = format!("{}{tail}", self.source);
        let trailing = joined.len() - joined.trim_end_matches('\n').len();
        for _ in trailing..2 {
            tail.push('\n');
        }
        if !tail.is_empty() && !self.splice(len, len, &tail, EditKind::Other) {
            return;
        }
        let end = self.source.len();
        self.move_to(end, false);
    }

    /// The closing fence a document ending inside an unclosed fenced code block
    /// needs — the opening fence's own run, behind the quote prefix the block
    /// wears — or `None` when the last block is closed, indented, or not a code
    /// block at all.
    ///
    /// Unclosed is when twig's content span reaches the block's end: a closing
    /// fence line would lie between the two. `code_info_span`'s read of the
    /// fence is not used because it starts at the block's span, which inside a
    /// quote is the quote marker rather than the fence.
    fn unclosed_fence_at_end(&mut self) -> Option<String> {
        let content_end = self.source.trim_end_matches('\n').len();
        let block = self
            .nodes()
            .into_iter()
            .filter(|n| n.kind == Kind::CodeBlock && n.span.end >= content_end)
            .max_by_key(|n| n.span.start)?;
        if block.content_span.as_ref()?.end < block.span.end {
            return None;
        }
        let line = self.source[block.span.start..].lines().next()?;
        let opening = line.trim_start_matches(['>', ' ', '\t']);
        let fence = opening.chars().next().filter(|c| matches!(c, '`' | '~'))?;
        let run: String = opening.chars().take_while(|&c| c == fence).collect();
        let prefix = self.quote_prefix_at(block.span.start);
        Some(format!("{prefix}{run}"))
    }

    /// Settle `scroll` for a frame about to be drawn: follow the caret onto the
    /// screen if it has moved since the last frame, and never scroll past the
    /// last of `rows`.
    ///
    /// Only if it has *moved* — that's the whole point. Revealing the caret on
    /// every frame ties the viewport to it, and a scroll wheel that fights the
    /// caret for the viewport loses: the view snaps back the instant it tries to
    /// pass the caret's row, so the document can't be scrolled beyond what's
    /// already on screen. A caret move is the frontend's cue to follow; a scroll
    /// with the caret sitting still is the reader's cue to leave it alone.
    pub fn follow_caret(&mut self, caret_row: usize, height: usize, rows: usize) {
        if self.drawn_caret != Some(self.caret) {
            if caret_row < self.scroll {
                self.scroll = caret_row;
            } else if height > 0 && caret_row >= self.scroll + height {
                self.scroll = caret_row + 1 - height;
            }
            self.drawn_caret = Some(self.caret);
        }
        self.scroll = self.scroll.min(rows.saturating_sub(1));
    }

    /// The caret's screen position `(row, col)` in the active view's grid, with
    /// `col` a display column: the cell to draw the caret in, which on a line of
    /// `你好` or emoji is not the count of characters before it.
    pub fn caret_pos(&self) -> (usize, usize) {
        match self.view {
            View::Source => offset_to_row_col(&self.source, self.caret),
            View::Wysiwyg => self.vmap.pos_of_offset(self.caret),
        }
    }

    pub(super) fn clamp_caret(&mut self) {
        if self.caret > self.source.len() {
            self.caret = self.source.len();
        }
        // In WYSIWYG the caret can't sit inside hidden frontmatter; lift it (and
        // any selection anchor) to the first rendered offset.
        let floor = self.caret_floor();
        if self.caret < floor {
            self.caret = floor;
        }
        if let Some(a) = self.anchor
            && a < floor
        {
            self.anchor = Some(floor);
        }
        while self.caret > 0 && !self.source.is_char_boundary(self.caret) {
            self.caret -= 1;
        }
    }
}
