//! Block kinds: setting and toggling headings, quotes, lists, verse, and code
//! blocks, and wrapping a selection in a block container.

use super::*;

impl Doc {
    /// Convert the block at the caret to a heading level or paragraph.
    pub fn set_block(&mut self, kind: BlockKind) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        if self.refuse_unsupported(&format!("{kind:?}"), Gesture::SetBlock) {
            return;
        }
        self.record_caret();
        // A blank line has no node to convert, and twig opens a block there
        // rather than declining — so the caret's own offset is the right thing
        // to hand it when `block_offset_for_caret` finds nothing.
        let offset = self.block_offset_for_caret().unwrap_or(self.caret);
        let before = self.source.clone();
        match self.editor.set_block(offset, kind) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                // Opening a block on a blank line writes a marker the caret
                // belongs *after*; converting an existing one rewrites the
                // block, its text kept, and the caret stays on that text.
                self.caret = caret_through(self.caret, &before, &self.source, &change);
                self.clamp_caret();
                self.anchor = None;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// Whether `off` is inside a text block (paragraph, heading, code block…).
    pub(super) fn has_block_at(&mut self, off: usize) -> bool {
        self.editor.ancestors_at(off).ok().is_some_and(|chain| {
            chain
                .iter()
                .any(|m| !wysiwyg::is_inline_kind(&m.kind) && !is_block_container(&m.kind))
        })
    }

    /// The offset to hand twig's `set_block`: the caret when it is already inside
    /// a block, otherwise nudged onto the previous character (a caret at a line
    /// end sits at the doc level, outside the block). `None` when the caret is on
    /// a blank line — a new paragraph with no block node to convert.
    pub(super) fn block_offset_for_caret(&mut self) -> Option<usize> {
        let caret = self.caret.min(self.source.len());
        if self.has_block_at(caret) {
            return Some(caret);
        }
        // Nudge to the previous character — but never across a newline: that would
        // target the previous block, and a blank line genuinely has no block.
        if let Some((i, ch)) = self.source[..caret].char_indices().next_back()
            && ch != '\n'
            && self.has_block_at(i)
        {
            return Some(i);
        }
        None
    }

    /// The heading level of the text block at the caret, or `None` when that
    /// block is not a heading.
    pub fn current_heading_level(&mut self) -> Option<u32> {
        let caret = self.caret;
        self.nodes()
            .into_iter()
            .filter(|n| n.kind == Kind::Heading)
            .find(|n| n.span.start <= caret && caret <= n.span.end)
            .and_then(|n| n.level)
    }

    /// The inline marks in force at the caret (or over the selection) — what a
    /// toolbar draws lit, and the block-level [`Doc::current_heading_level`]'s
    /// inline counterpart. Cheap enough to call every frame: one twig
    /// `ancestors_at` query per caret (two with a selection), each walking root
    /// → deepest node at one offset. It never snapshots the tree the way
    /// `current_heading_level` does, and the returned set is a `Copy` bitset, so
    /// the only allocation is twig's own small ancestor `Vec`.
    ///
    /// **A selection reports a mark only when the mark covers *all* of it.**
    /// That's what every real toolbar means by an active button — Bold lit over
    /// a half-bold selection would claim a press turns bold *off*, when
    /// [`Doc::toggle`] hands the range to twig and gets the whole thing bolded.
    /// Whole-coverage is asked as "is the same mark node standing over both the
    /// first and the last character?": inline nodes are contiguous, so one node
    /// covering both ends covers every byte between them. Two touching runs
    /// (`**a****b**`) are two nodes, and correctly light nothing.
    ///
    /// At a bare caret a mark is active when the caret stands inside the mark's
    /// span — `span.start <= caret < span.end`, delimiters included, which is
    /// what makes the boundaries behave. In `a **bold** b` the offsets from the
    /// opening `*` (2) through the last byte of the closing `**` (9) are all
    /// bold, so the WYSIWYG caret both before `b` and after `d` (the delimiters
    /// are hidden, and those offsets are 4 and 8) reports bold — matching where
    /// typing would actually land inside the marked run. The offset one past the
    /// mark (10) is the text after it and reports nothing, at the end of the
    /// buffer exactly as in the middle.
    pub fn active_inline_marks(&mut self) -> InlineMarks {
        let Some((start, end)) = self.selection() else {
            // The marks actually in force at the caret, flipped by any armed
            // sticky delta — so `⌘b` at a bare caret lights the Bold button
            // immediately, before a single character is typed.
            let base: InlineMarks = self
                .marks_at(self.caret)
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            return base.xor(self.pending_here());
        };
        // The selection's *last character*, not its exclusive end: `end` is the
        // offset one past the selection, which for a selection ending exactly at
        // a mark's close is already outside it (`[4,10)` of `a **bold** b` is
        // entirely bold, but offset 10 is the space after).
        let last = prev_boundary(&self.source, end);
        let head = self.marks_at(start);
        let tail = self.marks_at(last);
        head.into_iter()
            .filter(|m| tail.contains(m))
            .map(|(k, _)| k)
            .collect()
    }

    /// The inline marks whose span covers `off`, each with the id of the node
    /// carrying it — the id is what lets a selection tell one mark node from
    /// another of the same kind.
    pub(super) fn marks_at(&mut self, off: usize) -> Vec<(InlineKind, u32)> {
        let off = off.min(self.source.len());
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            // `span.end` is the offset one *past* the mark, so it isn't in it.
            // twig already resolves a boundary to whatever starts there — in
            // `**bold** x` offset 8 is the following text, not the strong — but
            // when nothing follows, the tie has nobody to break for and the
            // chain still ends at the mark. That would make the answer at the
            // last offset of the document depend on whether the file happens to
            // end in a newline; the rule is `span.start <= off < span.end`, and
            // it's the same rule at the end of a buffer as in the middle.
            .filter(|m| off < m.span.end)
            .filter_map(|m| inline_kind(&m.kind).map(|k| (k, m.node_id)))
            .collect()
    }

    /// Toggle a heading at the caret: if the block is already this heading level,
    /// revert it to a paragraph; otherwise convert it to this heading level.
    /// This gives the heading commands the same toggle feel as bold/italic/code —
    /// re-applying a heading a line already has turns it back into body text.
    pub fn toggle_heading(&mut self, level: u32) {
        if self.current_heading_level() == Some(level) {
            self.set_block(BlockKind::Paragraph);
        } else {
            self.set_block(BlockKind::Heading(level));
        }
    }

    /// Toggle a block quote around the selection, or around the block at the
    /// caret — the toolbar's Quote button.
    pub fn toggle_blockquote(&mut self) {
        self.toggle_container(BlockContainerKind::BlockQuote);
    }

    /// Toggle a numbered (`ordered`) or bulleted list over the selection, or
    /// over the block at the caret — one op with the kind as a flag, the way
    /// `toggle_heading` takes its level, so a frontend needs no twig type to
    /// name the two buttons.
    ///
    /// Pressing the *other* list's button while in a list converts in place
    /// rather than nesting, so the pair reads as one three-state control
    /// (bulleted / numbered / neither) rather than two independent wrappers.
    pub fn toggle_list(&mut self, ordered: bool) {
        self.toggle_container(if ordered {
            BlockContainerKind::OrderedList
        } else {
            BlockContainerKind::BulletList
        });
    }

    /// Make the paragraphs the selection touches — or the caret's paragraph —
    /// a verse, or the verse the caret is in back into prose: the Format
    /// menu's Verse item.
    ///
    /// The shape is twig's ([`Editor::toggle_line_block`]): each paragraph a
    /// stanza, each line break inside it a line, a line's leading em spaces
    /// its indent; and back, a paragraph per stanza with the indents kept as
    /// em spaces, so the toggle round-trips. Markdown spells it
    /// `<div class="verse">`, djot `::: verse`, AsciiDoc `[verse]`.
    ///
    /// The caret stays on the same line of text at the same column, across the
    /// tag lines written above it or taken away.
    pub fn toggle_verse(&mut self) {
        if self.read_only {
            return;
        }
        if self.refuse_unsupported("verse", Gesture::ToggleLineBlock) {
            return;
        }
        let selected = self.selection();
        let (start, end) = match selected {
            Some(range) => range,
            None => {
                let off = self.block_offset_for_caret().unwrap_or(self.caret);
                (off, off)
            }
        };
        self.record_caret();
        let place = selected.is_none().then(|| self.verse_caret_place());
        match self.editor.toggle_line_block(start, end) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                match place.flatten() {
                    Some((line, col)) => {
                        self.anchor = None;
                        self.caret = self.verse_caret_at(&change.new, line, col);
                    }
                    None => {
                        self.anchor = Some(change.new.start);
                        self.caret = change.new.end.min(self.source.len());
                    }
                }
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("verse: {e}")),
        }
    }

    /// Where the caret is as a line of *text*, for [`toggle_verse`] to put it
    /// back: how many lines of text come before its own in the source above
    /// it, counted from the start of its block, and its column on the line.
    /// Blank lines and the tag or fence lines a verse is spelled with are not
    /// lines of text, which is what makes the count survive the toggle.
    ///
    /// [`toggle_verse`]: Self::toggle_verse
    fn verse_caret_place(&mut self) -> Option<(usize, usize)> {
        let block_start = self.verse_region_start(self.caret)?;
        let line_start = self.source[..self.caret].rfind('\n').map_or(0, |p| p + 1);
        let before = &self.source[block_start..line_start];
        let line = before
            .split_terminator('\n')
            .filter(|l| is_text_line(l))
            .count();
        Some((line, self.caret - line_start))
    }

    /// The start of the block the caret's toggle will rewrite: the verse
    /// around `off` if there is one, else the paragraph's first line.
    fn verse_region_start(&mut self, off: usize) -> Option<usize> {
        if let Some(lb) = self.verse_at(off) {
            return Some(lb.span.start);
        }
        let probe = self.block_offset_for_caret().unwrap_or(off);
        let chain = self.editor.ancestors_at(probe).ok()?;
        chain
            .iter()
            .rev()
            .find(|m| m.kind == Kind::Para)
            .map(|m| m.span.start)
    }

    /// The offset of column `col` on the `line`th line of text in `region`,
    /// clamped to that line's text — short of a trailing `\` break.
    fn verse_caret_at(&self, region: &std::ops::Range<usize>, line: usize, col: usize) -> usize {
        let end = region.end.min(self.source.len());
        let mut at = region.start.min(end);
        let mut seen = 0;
        for l in self.source[at..end].split('\n') {
            if is_text_line(l) {
                if seen == line {
                    let body = l.trim_end_matches('\r');
                    let body = body.strip_suffix('\\').unwrap_or(body);
                    let mut c = col.min(body.len());
                    while !body.is_char_boundary(c) {
                        c -= 1;
                    }
                    return at + c;
                }
                seen += 1;
            }
            at += l.len() + 1;
        }
        region.start.min(self.source.len())
    }

    /// The verse (`line_block`) whose span holds `off`, if any.
    fn verse_at(&mut self, off: usize) -> Option<twig::QueryMatch> {
        let chain = self.editor.ancestors_at(off.min(self.source.len())).ok()?;
        chain.into_iter().rev().find(|m| m.kind == Kind::LineBlock)
    }

    /// Enter inside a verse: a new line, the way a text editor makes one.
    ///
    /// At a line that ends in a `\` break the break is rewritten as a blank
    /// line and the caret put on it — an empty line between two lines, which
    /// the first character typed there makes a line. A newline written in
    /// front of the `\` would have left the `\` alone on the next line, where
    /// it breaks nothing, and the empty line the caret stood on would not
    /// exist. Everywhere else — mid-line, on a blank line, at a line with a
    /// plain newline after it — a newline is the line break itself.
    pub(super) fn verse_newline(&mut self) {
        let c = self.caret;
        let rest = &self.source[c..];
        let brk = if rest.starts_with("\\\r\n") {
            3
        } else if rest.starts_with("\\\n") {
            2
        } else {
            0
        };
        if brk == 0 {
            self.insert_raw("\n");
            return;
        }
        if self.splice(c, c + brk, "\n\n", EditKind::Other) {
            self.caret = c + 1;
            self.record_caret();
        }
    }

    /// Backspace at the start of a line inside a verse. `true` when it acted.
    ///
    /// Below the first line it joins the line to the one above by taking the
    /// newline that parts them, and the `\` before it that made the break a
    /// hard one — leaving the `\` would put a literal backslash between the
    /// two halves. A blank line above (a stanza break) goes the same way, one
    /// newline at a time. At the first line's start the verse turns back into
    /// prose ([`toggle_verse`](Self::toggle_verse)), as Backspace at a list's
    /// first item takes the list away.
    pub(super) fn backspace_verse(&mut self) -> bool {
        let c = self.caret;
        if c == 0 || !self.source[..c].ends_with('\n') {
            return false;
        }
        let Some(lb) = self.verse_at(c) else {
            return false;
        };
        let first_line = self.editor.nodes().ok().and_then(|nodes| {
            let mut child = nodes.get(lb.node_id as usize)?.first_child;
            while let Some(id) = child {
                let n = nodes.get(id.0 as usize)?;
                if n.first_child.is_some() {
                    return Some(n.span.start);
                }
                child = n.next_sibling;
            }
            None
        });
        let Some(first_line) = first_line else {
            return false;
        };
        if c < first_line {
            return false;
        }
        if c == first_line {
            self.toggle_verse();
            return true;
        }
        let mut from = c - 1;
        if self.source[..from].ends_with('\r') {
            from -= 1;
        }
        let above = &self.source[..from];
        if above.ends_with('\\') && !above.ends_with("\\\\") {
            from -= 1;
        }
        if self.splice(from, c, "", EditKind::Delete) {
            self.caret = from;
            self.record_caret();
        }
        true
    }

    /// Tab / Shift+Tab inside a verse: one em space more, or one fewer, at the
    /// start of each selected line — the caret's line with no selection. `true`
    /// when the caret is in a verse, whether or not a line moved.
    ///
    /// One splice for the whole range, as [`reindent`](Self::reindent) makes,
    /// so a Tab is one undo step. A blank line is skipped: it has no text to
    /// indent, and an em space on it would make it a line.
    pub(super) fn verse_reindent(&mut self, add: bool) -> bool {
        if self.view == View::Source {
            return false;
        }
        let (sel_start, sel_end) = self.selection().unwrap_or((self.caret, self.caret));
        let Some(lb) = self.verse_at(sel_start) else {
            return false;
        };
        let start = source_line_range(&self.source, sel_start)
            .start
            .max(lb.span.start);
        let end = source_line_range(&self.source, sel_end)
            .end
            .min(lb.span.end);
        if start >= end {
            return true;
        }
        let region = self.source[start..end].to_string();
        let mut out = String::with_capacity(region.len() + 8);
        let mut caret_delta: isize = 0;
        let mut anchor_delta: isize = 0;
        let caret_line = source_line_range(&self.source, self.caret).start;
        let anchor_line = self
            .anchor
            .map(|a| source_line_range(&self.source, a).start);
        let mut line_off = start;
        for (i, line) in region.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let delta: isize = if !is_text_line(line) {
                out.push_str(line);
                0
            } else if add {
                out.push('\u{2003}');
                out.push_str(line);
                '\u{2003}'.len_utf8() as isize
            } else if let Some(n) = crate::wysiwyg::em_space_at(line, 0) {
                out.push_str(&line[n..]);
                -(n as isize)
            } else {
                out.push_str(line);
                0
            };
            if line_off <= caret_line
                && caret_line <= line_off + line.len()
                && line_off == caret_line
            {
                caret_delta = delta;
            }
            if anchor_line == Some(line_off) {
                anchor_delta = delta;
            }
            line_off += line.len() + 1;
        }
        if out == region {
            return true;
        }
        let caret = self.caret;
        let anchor = self.anchor;
        if self.splice(start, end, &out, EditKind::Other) {
            self.caret = (caret as isize + caret_delta).max(start as isize) as usize;
            self.anchor = anchor.map(|a| (a as isize + anchor_delta).max(start as isize) as usize);
            self.clamp_caret();
            self.record_caret();
        }
        true
    }

    /// Toggle a fenced code block over the selection, or over the block at the
    /// caret — the toolbar's Code Block button, and the only way the rich view
    /// offers to open one: a typed backtick is escaped there, since it is a
    /// character the author wrote and not markup they meant.
    ///
    /// Fencing is twig's ([`Editor::toggle_code_block`]): it measures the fence
    /// against the body, keeps a quote's `> ` on every line, and peels a fence
    /// (or dedents an indented block) on the way back. What is leaf's is the
    /// shape twig declines: a blank line holds no block to fence, and the
    /// gesture nobody should have to learn is "type something first" — so leaf
    /// spells the empty fence itself, with the caret on the empty line inside it
    /// and a blank line either side, which is what typing into a new block and
    /// then Backspacing out of it would have left.
    ///
    /// Inside a list item twig fences at the item's content column and keeps
    /// the marker (a task item's box too) on the opening fence's line, so the
    /// block stays the item's; unfencing puts the marker back on the text.
    /// What twig still refuses — an AsciiDoc item's own first line — is
    /// reported rather than worked around.
    pub fn toggle_code_block(&mut self) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        if self.refuse_unsupported("code block", Gesture::ToggleCodeBlock) {
            return;
        }
        let selected = self.selection();
        // The caret at a code block's rows resolves to its *content*, and twig
        // finds the block from any offset inside its span — but a caret parked
        // on the closing fence's own line end is past it, so the block's start
        // is handed over whenever the caret is in one at all.
        let (start, end) = match selected {
            Some(range) => range,
            None => match self.code_block_start_at_caret() {
                Some(start) => (start, start),
                None => match self.block_offset_for_caret() {
                    Some(off) => (off, off),
                    None => return self.open_empty_code_block(),
                },
            },
        };
        self.record_caret();
        match self.editor.toggle_code_block(start, end, None) {
            Ok(change) => {
                // Read the caret's place out of the *pre-edit* source, before
                // `refresh` swaps it out — and how many lines the region held,
                // which is what says whether a fence line went in above the
                // caret's line or came off it.
                let place = selected
                    .is_none()
                    .then(|| self.caret_line_tail(&change.old));
                let old_lines = self.source[change.old.clone()].matches('\n').count();
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                match place {
                    // From a selection: keep the block selected, so a second
                    // press reverses the first (twig unfences from any offset in
                    // the fence's span, the region's start included).
                    None => {
                        self.anchor = Some(change.new.start);
                        self.caret = change.new.end;
                    }
                    // From a caret: the same line, the same distance from its
                    // end, shifted by the opening fence that was written above
                    // it (or peeled off). Dedenting an indented block keeps the
                    // lines one-to-one, and shifts nothing.
                    Some((line, tail)) => {
                        let new_lines = self.source[change.new.start.min(self.source.len())
                            ..change.new.end.min(self.source.len())]
                            .matches('\n')
                            .count();
                        let line = match new_lines.cmp(&old_lines) {
                            std::cmp::Ordering::Greater => line + 1,
                            std::cmp::Ordering::Less => line.saturating_sub(1),
                            std::cmp::Ordering::Equal => line,
                        };
                        self.anchor = None;
                        self.caret = self.line_tail_offset(&change.new, (line, tail));
                    }
                }
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("code block: {e}")),
        }
    }

    /// Write an empty fenced block on the blank line the caret stands on, and
    /// put the caret on the empty line inside it — the half of
    /// [`toggle_code_block`](Self::toggle_code_block) that is leaf's, because
    /// twig reports `NotFound` for a range no block covers.
    ///
    /// Inside a quote the fence lines and the empty line keep the quote's
    /// prefix, as twig's own fencing does. A blank line goes in on whichever
    /// side has text against it, since a fence may interrupt a paragraph in
    /// Markdown but a block standing tight against its neighbours is not the
    /// document any editor writes.
    fn open_empty_code_block(&mut self) {
        let caret = self.caret.min(self.source.len());
        let prefix = self.quote_prefix_at(caret);
        let line_start = self.source[..caret].rfind('\n').map_or(0, |i| i + 1);
        let line_end = self.source[caret..]
            .find('\n')
            .map_or(self.source.len(), |i| caret + i);
        let prev_blank = line_start == 0
            || self.source[..line_start - 1]
                .rsplit('\n')
                .next()
                .is_some_and(|l| l.trim_start_matches(['>', ' ']).is_empty());
        let next_blank = line_end >= self.source.len()
            || self.source[line_end + 1..]
                .split('\n')
                .next()
                .is_some_and(|l| l.trim_start_matches(['>', ' ']).is_empty());
        // A blank line inside a quote is a bare `>`: the space after it is
        // the content's, and there is none.
        let blank = prefix.trim_end();
        let mut text = String::new();
        if !prev_blank {
            text.push_str(blank);
            text.push('\n');
        }
        text.push_str(&prefix);
        text.push_str("```\n");
        text.push_str(&prefix);
        let inside = text.len();
        text.push('\n');
        text.push_str(&prefix);
        text.push_str("```");
        if !next_blank {
            text.push('\n');
            text.push_str(blank);
        }
        // Over whatever the blank line held (its quote prefix, trailing
        // spaces), so the block's own prefix is the one twig will read back.
        let at = line_start;
        if !self.splice(at, line_end, &text, EditKind::Other) {
            return;
        }
        self.caret = at + inside;
        self.anchor = None;
        self.clamp_caret();
        self.record_caret();
    }

    /// Whether the caret stands in a code block, fenced or indented — what
    /// lights the Code Block button. Wider than
    /// [`caret_in_fenced_code`](Self::caret_in_fenced_code), which asks the
    /// narrower question a language prompt needs.
    pub fn caret_in_code_block(&mut self) -> bool {
        self.code_block_start_at_caret().is_some()
    }

    /// Whether the caret stands in a verse — what ticks the Verse item.
    pub fn caret_in_verse(&mut self) -> bool {
        self.verse_at(self.caret).is_some()
    }

    /// Whether the caret stands anywhere inside a block quote, however deep —
    /// what lights the Block Quote button. A quote is a container rather than
    /// a block kind, so this is true alongside a heading or a code block, not
    /// instead of one.
    pub fn caret_in_blockquote(&mut self) -> bool {
        let off = self.caret.min(self.source.len());
        // A quote's span stops short of its closing newline, so the end of its
        // last line is `span.end` itself, and has to count.
        self.nodes()
            .into_iter()
            .any(|n| n.kind == Kind::BlockQuote && n.span.start <= off && off <= n.span.end)
    }
}

impl Doc {
    /// One `toggle_block_container` over the block-level target.
    ///
    /// leaf says *where*; twig decides everything else — which blocks the range
    /// covers, whether that means wrapping, unwrapping, nesting or converting,
    /// and how this document's format spells the prefix. The rule that a
    /// container only comes off when the range covers every block it holds is
    /// what the re-anchoring below is built around.
    fn toggle_container(&mut self, kind: BlockContainerKind) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        if self.refuse_unsupported(&format!("{kind:?}"), Gesture::ToggleBlockContainer(kind)) {
            return;
        }
        let selected = self.selection();
        // A blank line holds no block, and twig opens an *empty* container on one
        // — since 3.2.0; it used to decline the range with `NotFound`, which is
        // why this used to lend it a scratch paragraph to wrap. Worth knowing
        // here because the line-for-line caret mapping below cannot describe it:
        // opening one under a paragraph writes the blank line the format needs
        // above the marker too, so the rewritten region has a line the old one
        // didn't, and "the same line, the same distance from its end" lands on
        // that new blank instead of in the container.
        let opened_empty = selected.is_none() && self.block_offset_for_caret().is_none();
        // Without a selection the target is the caret's own block, resolved the
        // way `set_block` resolves it — a caret at a line end sits at the doc
        // level and has to be nudged back onto the block it looks like it's in.
        // An empty range is enough: twig widens to the whole lines it touches.
        let (start, end) = match selected {
            Some(range) => range,
            None => {
                let off = self.block_offset_for_caret().unwrap_or(self.caret);
                (off, off)
            }
        };
        self.record_caret();
        match self.editor.toggle_block_container(start, end, kind) {
            Ok(change) => {
                // Read the caret's place out of the *pre-edit* source, before
                // `refresh` swaps that source out from under it.
                let place = (selected.is_none() && !opened_empty)
                    .then(|| self.caret_line_tail(&change.old));
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                match place {
                    // Both land the caret at the far end of what twig wrote, and
                    // differ only in what they leave selected.
                    //
                    // From a selection: select what the container now holds, the
                    // way `toggle` keeps its marked region selected — and for a
                    // stronger reason than symmetry: a container comes *off* only
                    // a range covering every block it holds, so a selection left
                    // on its old bytes (now short by a prefix per line) would nest
                    // on the second press instead of reversing the first.
                    //
                    // From a blank line: nothing to select, and the end of the
                    // region is exactly past the bare `> ` / `- ` twig wrote —
                    // the caret standing inside the container that was asked for.
                    None => {
                        self.anchor = (!opened_empty).then_some(change.new.start);
                        self.caret = change.new.end;
                    }
                    Some(place) => {
                        self.anchor = None;
                        self.caret = self.line_tail_offset(&change.new, place);
                    }
                }
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// The caret's place inside the region a container toggle is rewriting, in
    /// the only terms the rewrite preserves: which of the region's lines it sits
    /// on, and how many bytes of that line lie ahead of it.
    ///
    /// A container's markup goes in at column 0 and never touches what follows
    /// on the line, so that pair survives the edit exactly where a byte offset
    /// does not — a caret left on its old offset slides back by one prefix per
    /// line above it, which on a hard-wrapped paragraph parks it *inside* the
    /// `> ` it just asked for.
    fn caret_line_tail(&self, old: &std::ops::Range<usize>) -> (usize, usize) {
        let caret = self.caret.clamp(old.start, old.end);
        let line = self.source[old.start..caret].matches('\n').count();
        let end = self.source[caret..old.end]
            .find('\n')
            .map_or(old.end, |i| caret + i);
        (line, end - caret)
    }

    /// [`caret_line_tail`](Self::caret_line_tail) undone against the rewritten
    /// region: the offset `tail` bytes back from the end of the region's `line`.
    ///
    /// Both walks are clamped rather than trusted, because the one op that does
    /// *not* keep a region's lines one-to-one is stripping a list — twig blows
    /// the items back apart with blank lines between them — and a caret landing
    /// on the nearest line of the right item beats one landing out of the region
    /// entirely.
    fn line_tail_offset(
        &self,
        new: &std::ops::Range<usize>,
        (line, tail): (usize, usize),
    ) -> usize {
        let region = &self.source[new.start.min(self.source.len())..new.end.min(self.source.len())];
        let mut start = 0;
        for _ in 0..line {
            match region[start..].find('\n') {
                Some(i) => start += i + 1,
                None => break,
            }
        }
        let end = region[start..]
            .find('\n')
            .map_or(region.len(), |i| start + i);
        new.start + end.saturating_sub(tail).max(start)
    }
}

/// Where a caret at `caret` in `before` is after `change` made `after`:
/// before the change, where it was; after it, moved by it; in it, on the
/// same text if the text after the caret is the rewritten block's own tail
/// (a heading's marker added or taken off its paragraph), else after what
/// was written (a marker opened on a blank line).
fn caret_through(caret: usize, before: &str, after: &str, change: &twig::Change) -> usize {
    let (old, new) = (&change.old, &change.new);
    if caret < old.start {
        return caret;
    }
    if caret > old.end {
        return (caret as isize + change.delta()).max(0) as usize;
    }
    let (Some(was), Some(is)) = (before.get(old.clone()), after.get(new.clone())) else {
        return caret.max(new.end);
    };
    let tail = was
        .bytes()
        .rev()
        .zip(is.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(was.len())
        .min(is.len());
    let from_end = old.end - caret;
    if !was.trim().is_empty() && from_end <= tail {
        let at = new.end - from_end;
        if after.is_char_boundary(at) {
            return at;
        }
    }
    caret.max(new.end)
}
