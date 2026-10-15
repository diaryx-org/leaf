//! Turning glyphs into rows: wrapping, prefixes, code lines, and the blank
//! lines a block keeps around it.

use super::*;

impl Builder<'_> {
    /// Lay a block's inline `glyphs` into visual rows, prefixing the first with
    /// `pf` and the rest with `pc`. A preserved soft break arrives as a `'\n'`
    /// glyph (see the `soft_break` arm): a hard row boundary that splits the
    /// glyphs so each run lays out on its own and the author's line structure
    /// shows on screen. The `'\n'` is dropped from the row it closes and its
    /// source offset becomes that row's end stop — exactly how a table cell's
    /// in-line `<br>` is handled — so the caret can rest at the line's end
    /// without a zero-width control char leaking into what the frontends render.
    /// With no `'\n'` present (the folding default, and every build that isn't
    /// `LineFlow::Preserve`) there is one run and this is byte-identical to
    /// laying the glyphs out directly.
    pub(super) fn emit_wrapped(
        &mut self,
        glyphs: Vec<Glyph>,
        block_start: usize,
        pf: &[Glyph],
        pc: &[Glyph],
    ) {
        if !glyphs.iter().any(|g| g.ch == '\n') {
            self.emit_line(glyphs, block_start, pf, pc, None);
            return;
        }
        // Each run up to a '\n' is a line of its own: the first wears the block's
        // opening prefix, every later one the continuation prefix, and the break's
        // own offset ends the run's last row. The break glyph is dropped. A
        // trailing '\n' flushes its run and leaves nothing behind, so no spurious
        // blank row follows it.
        let mut run: Vec<Glyph> = Vec::new();
        let mut first = true;
        for g in glyphs {
            if g.ch == '\n' {
                let lead = if first { pf } else { pc };
                self.emit_line(std::mem::take(&mut run), block_start, lead, pc, Some(g.src));
                first = false;
            } else {
                run.push(g);
            }
        }
        if !run.is_empty() {
            let lead = if first { pf } else { pc };
            self.emit_line(run, block_start, lead, pc, None);
        }
    }

    /// Word-wrap a single line of `glyphs` (no interior line breaks) to the
    /// available width and push the visual rows, prefixing the first with `pf`
    /// and the rest with `pc`. `end`, when set, is the source offset that ends
    /// the line's final row — the offset of the break that terminated it, which
    /// the caller has already stripped from `glyphs`; when `None` the row ends
    /// just past its last glyph, as an unbroken block's does.
    fn emit_line(
        &mut self,
        glyphs: Vec<Glyph>,
        block_start: usize,
        pf: &[Glyph],
        pc: &[Glyph],
        end: Option<usize>,
    ) {
        // The line's final row ends at `end` when a break gave one, else just
        // past its last glyph (`push_row`'s default).
        let push_last = |b: &mut Self, row: Vec<Glyph>| match end {
            Some(e) => b.push_row_at(row, e),
            None => b.push_row(row, block_start),
        };

        // No column budget: emit the whole line as one row and let the frontend
        // wrap it at its own (pixel) width.
        let Some(width) = self.wrap else {
            let row = if glyphs.is_empty() {
                pf.to_vec()
            } else {
                concat(pf, &glyphs)
            };
            push_last(self, row);
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
            // An empty block (or an empty preserved line) still occupies one
            // (prefixed) row.
            push_last(self, pf.to_vec());
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
        push_last(self, row);
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
    pub(super) fn code_line_offsets(
        &self,
        content: &Range<usize>,
        lines: &[&str],
    ) -> Option<Vec<usize>> {
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
        // Step past the character the *source* holds at the last glyph's offset,
        // not past the glyph's own `ch`. The two agree for ordinary text, but a
        // glyph is not always the character it stands on: `synth` decoration and
        // a substituted run (an image's `⧉ label`) share one offset by design.
        // Trusting `ch` there yields an offset inside a multi-byte character,
        // which every later slice of `source` panics on.
        let end_src = glyphs
            .last()
            .map(|g| {
                let at = g.src.min(self.source.len());
                at + self.source[at..].chars().next().map_or(0, char::len_utf8)
            })
            .unwrap_or(fallback);
        self.push_row_at(glyphs, end_src);
    }

    /// Push a row with an explicit end stop, for content that knows its own
    /// extent better than its last glyph does.
    pub(super) fn push_row_at(&mut self, glyphs: Vec<Glyph>, end_src: usize) {
        self.last_off = end_src;
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

    /// Where the last row's line ends — what the lines under it are counted
    /// from: the row's own end, or the walk's where that stands short of it.
    /// Only a thematic break's does, whose row ends at the caret's home past
    /// the newline under the rule while the walk stands at the rule itself
    /// (see its arm). Every other block leaves the walk at its last row's end
    /// or past it — past a closing fence, say, which the counts below reach
    /// by other means.
    fn last_line_end(&self) -> Option<usize> {
        self.rows.last().map(|r| r.end_src.min(self.last_off))
    }

    /// The quote's own trailing marker lines: the `>` / `> ` lines that lie past
    /// its last child but inside its span, one gutter row each.
    ///
    /// Pressing Enter at the end of `> a` writes `> a\n>\n> \n` — twig's
    /// spelling, and the right one. Those last two lines hold no block (a
    /// `block_quote`'s `content_span` still stops at its last child) so the
    /// children walk never reaches them, and they used to fall all the way to
    /// the document-level [`Builder::emit_trailing_blank_lines`], which knows no
    /// prefix: the gutter simply stopped, and a writer adding a line to a quote
    /// watched it draw as plain prose.
    ///
    /// This is only answerable since twig 3.2.0, where a Markdown `block_quote`'s
    /// span covers its own trailing marker lines (it reported `0..3` for that
    /// source and now reports `0..8`). Before that the lines belonged to no node
    /// at any level, and the only way to draw them was to sniff `>` off the raw
    /// source and re-derive the nesting depth by counting markers — format
    /// inference this crate exists to keep out of the render path.
    ///
    /// Each row is a real caret home rather than a decoration gap: the writer
    /// spelled every one of these lines with a marker of its own, so each is a
    /// line of the quote to stand on, not the spacing between two blocks (which
    /// is [`Builder::emit_separators_before`]'s, and falls *between* children
    /// where this never looks).
    pub(super) fn emit_quote_trailing_lines(&mut self, pc: &[Glyph], end: usize) {
        let end = end.min(self.source.len());
        // From where the walk stands, not where the last row ends: a fenced
        // code block's last row is its last line of code, and its closing
        // fence (`> ```) is markup the block already stepped `last_off` past.
        // Counted from the row, the fence's line drew as an empty quoted line.
        let mut at = self.last_line_end().unwrap_or(0).max(self.last_off);
        // Walk line by line from the last child's end to the quote's, taking each
        // line's *end* as the row's offset — the caret home at the end of a line
        // is where one on an empty quoted line belongs, and it keeps every row's
        // offset distinct from its neighbours'.
        while at < end {
            let Some(k) = self.source[at..end].find('\n') else {
                break;
            };
            let line_start = at + k + 1;
            let line_end = self.source[line_start..end]
                .find('\n')
                .map_or(end, |i| line_start + i);
            self.push_row_at(pc.to_vec(), line_end);
            at = line_end;
        }
    }

    /// The number the paragraph holding node `id` opens with — the `20` of
    /// `*20* And it came to pass…`, past any emphasis the number is set in.
    /// `None` when the paragraph opens with anything else, or `id` is in none.
    pub(super) fn opening_number(&self, id: usize) -> Option<&str> {
        let mut at = self.nodes.get(id)?.parent;
        let para = loop {
            let node = &self.nodes[at?.0 as usize];
            if node.kind == Kind::Para {
                break node;
            }
            at = node.parent;
        };
        let text = self.source.get(para.span.clone())?;
        let text = text.trim_start_matches(|c: char| c == '*' || c == '_' || c.is_whitespace());
        let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        (digits > 0).then(|| &text[..digits])
    }

    /// The source offset the caret rests at on the blank line separating a block
    /// that ends at `prev_end` from the next block starting at `next_start`:
    /// just past the newline that terminates the previous block, but kept
    /// strictly before the next block so the offset is unique to this row.
    pub(super) fn blank_line_offset(&self, prev_end: usize, next_start: usize) -> usize {
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
    ///
    /// Only a line that *is* blank counts. A line of markup between the two —
    /// a Djot block attribute (`{#v21}`) over the block it names, which the
    /// block's span starts below — is the next block's, not a gap: counted, it
    /// drew a second boundary row under every attributed block, and a
    /// scripture chapter whose every verse carries an anchor sat a full line
    /// apart where its paragraphs should be half of one.
    pub(super) fn blank_rows_between(&self, prev_end: usize, next_start: usize) -> Vec<usize> {
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
        let next_line_start = self.source[..next_start].rfind('\n').map_or(0, |p| p + 1);
        let mut offs = Vec::new();
        let mut start = prev_end + nl + 1;
        while start < next_line_start {
            let line = self.source[start..next_line_start]
                .lines()
                .next()
                .unwrap_or("");
            if line.trim().is_empty() {
                offs.push(start);
            }
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
    ///
    /// `above` is the class of the last block in the document — the one this gap
    /// closes. A document with no blocks at all has nothing above these rows, and
    /// [`BlockClass::Paragraph`] is the honest answer there too: what they are is
    /// empty paragraphs, on both sides of the gap.
    pub(in crate::wysiwyg) fn emit_trailing_blank_lines(
        &mut self,
        above: BlockClass,
        hidden_end: usize,
    ) {
        // With no rows at all the count starts past any hidden frontmatter, not
        // at 0: its newlines are not trailing blank lines, and counting them
        // opened phantom rows *inside* the metadata for a frontmatter-only file.
        //
        // Or past the last hidden block, if that is later: a closing comment
        // draws no row, and its lines are not blank lines the author opened.
        //
        // Or past the last byte of content, if *that* is later: a fenced code
        // block's last row ends at its last line of code, and the closing
        // fence under it is markup with no row of its own — so counted from
        // the row, the fence's own line and terminator read as two blank lines
        // and opened a phantom empty paragraph whose offset was *inside* the
        // fence. Typing on it broke the fence. (A setext heading's underline
        // was the same shape.) Trailing whitespace is not content, so a line
        // of spaces still counts as the blank line it looks like.
        let last_end = self
            .last_line_end()
            .unwrap_or(hidden_end)
            .max(self.stepped_over)
            .max(self.source.trim_end().len());
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
                // one the way a following block would. Preserve flow makes even
                // that first row navigable, as it does every blank line.
                decoration: !self.preserve_soft && k == 1,
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
                // The one drawn row here is a block boundary like any other —
                // "rendered the way a block boundary is rendered" is the whole
                // point of it — so it says so, and a frontend spacing boundaries
                // spaces this one the same. The rows below it are navigable empty
                // paragraphs, not gaps.
                boundary: (!self.preserve_soft && k == 1).then_some(Boundary {
                    above,
                    below: BlockClass::Paragraph,
                }),
                mark_ends: Vec::new(),
                math: Vec::new(),
                region: None,
                inline_regions: Vec::new(),
            });
        }
    }
}
