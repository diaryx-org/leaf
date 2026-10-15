//! Inserting things that aren't typed text: links, images, media, thematic
//! breaks, and tables.

use super::*;

impl Doc {
    /// Link the selection to `destination` — the toolbar's Link button. With no
    /// selection it acts at the caret, which re-points a link the caret is
    /// already standing in (twig replaces an existing link's destination and
    /// keeps its text) and otherwise spells a link that has no text of its own:
    /// an autolink (`<https://x.dev>`) where the destination is one, and
    /// `[destination](destination)` where it isn't.
    ///
    /// `destination` reaches twig raw. Escaping it is format knowledge and the
    /// two formats genuinely disagree — Markdown ends a destination at the first
    /// space and moves it into `<…>`, djot reads that `<…>` as part of the URL
    /// itself — so the side holding the document is the side that gets to spell
    /// it. A destination twig can't carry at all (one with a newline) comes back
    /// as an error rather than a quietly rewritten URL.
    pub fn insert_link(&mut self, destination: &str) {
        if self.read_only || self.refuse_unsupported("link", Gesture::InsertLink) {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        self.record_caret();
        match self.editor.insert_link(start, end, destination) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                match self.link_text_span(change.new.start) {
                    // A link with text of its own: select it, so typing replaces
                    // a `[dest](dest)`'s stand-in label and a second press
                    // re-points what the first one linked.
                    Some(text) => {
                        self.anchor = (text.start != text.end).then_some(text.start);
                        self.caret = text.end;
                    }
                    // An autolink is finished the moment it's written — its text
                    // *is* the URL. Leaving it selected would aim the next press
                    // at the one shape twig still wraps instead of re-points.
                    None => {
                        self.anchor = None;
                        self.caret = change.new.end;
                    }
                }
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("link: {e}")),
        }
    }

    /// Insert a link to `destination` whose text is `label` — for a host that
    /// links something it has just made and knows the name of, where
    /// [`insert_link`](Self::insert_link)'s `[dest](dest)` would leave a file
    /// name where a title belongs.
    ///
    /// With no selection, `label` is written at the caret as the link's text
    /// and the caret lands just past the link, nothing selected: the link is
    /// finished, not a stand-in to type over. The label goes in through twig's
    /// `insert_literal`, so it is escaped in the body's own grammar the way the
    /// destination is — a `]` or `*` in a title stays that character rather
    /// than closing the link or opening emphasis. The label and the link are
    /// one undo step.
    ///
    /// Everywhere else this is [`insert_link`](Self::insert_link) and the
    /// label is ignored: with a selection, which is linked as it stands; with
    /// a caret standing inside a link or an autolink, which is re-pointed and
    /// keeps the text it has, so a host's Edit Link can call this one verb for
    /// both; and with an empty `label`.
    pub fn insert_link_labelled(&mut self, destination: &str, label: &str) {
        if self.selection().is_some() || label.is_empty() {
            return self.insert_link(destination);
        }
        if self.read_only || self.refuse_unsupported("link", Gesture::InsertLink) {
            return;
        }
        let caret = self.caret;
        if self
            .nodes()
            .into_iter()
            .any(|n| n.kind == Kind::Link && n.span.start < caret && caret < n.span.end)
        {
            return self.insert_link(destination);
        }
        self.record_caret();
        let text = match self.editor.insert_literal(caret, label) {
            Ok(change) => change.new,
            Err(e) => {
                self.status = Some(format!("link: {e}"));
                return;
            }
        };
        match self.editor.insert_link(text.start, text.end, destination) {
            Ok(change) => {
                // Two twig steps, the label's and the link's: each counted as
                // `refresh` counts an edit, then folded into one, so the label
                // is never on the history without its link. Inside a host's
                // undo group the second `refresh` has folded it already.
                self.last_edit_kind = None;
                self.refresh();
                self.refresh();
                self.coalesce_last_undo();
                self.anchor = None;
                self.caret = change.new.end;
                self.goal_col = None;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => {
                // Take the label back out, so a destination twig refuses
                // leaves the document as it found it. Writing the label ended
                // whatever was undone before it, so nothing is left to redo.
                let _ = self.editor.undo();
                self.redo_steps = 0;
                self.status = Some(format!("link: {e}"));
            }
        }
    }

    /// Insert a block-level image at the caret: `![alt](destination)`. Any
    /// selection becomes the alt text (so "select a caption, insert image" labels
    /// it); with no selection, `alt` is used — empty for none. The caret lands
    /// just past the inserted image.
    ///
    /// Both halves go through twig (`insert_literal` for the alt text,
    /// `insert_image` for the image), so neither is spelled here. That used to be a
    /// `format!`, and it was wrong the first time an app inserted a real filename:
    /// Markdown ends a destination at the first space, so `![](my photo.png)` is
    /// not an image at all — and the fix is per-format, since moving into the
    /// `<…>` form is exactly wrong for Djot, where `<…>` becomes the URL itself.
    pub fn insert_image(&mut self, destination: &str, alt: &str) {
        if self.read_only || self.refuse_unsupported("image", Gesture::InsertImage) {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        self.record_caret();
        // With no selection and an explicit `alt`, the alt text has to exist in the
        // document before it can be the image's — and it is raw caller input, so
        // it goes in through `insert_literal`, which escapes it for the format
        // rather than letting a `]` in someone's caption close the image early.
        let (start, end) = if start == end && !alt.is_empty() {
            match self.editor.insert_literal(start, alt) {
                Ok(change) => (change.new.start, change.new.end),
                Err(e) => {
                    self.status = Some(format!("image: {e}"));
                    return;
                }
            }
        } else {
            (start, end)
        };
        match self.editor.insert_image(start, end, destination) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                // Just past the image, nothing selected — where a caret belongs
                // after inserting one.
                self.anchor = None;
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("image: {e}")),
        }
    }

    /// Insert a block-level image, video, or audio at the caret. The image case
    /// is [`insert_image`](Self::insert_image); video and audio are spelled as
    /// HTML elements, which is the only spelling Markdown and Djot have for them:
    ///
    /// ```text
    /// <video src="clip.mp4" controls>alt</video>
    /// <audio src="take.mp3" controls>alt</audio>
    /// ```
    ///
    /// HTML rather than a `::video{…}` directive deliberately. A directive means
    /// something only to an app that knows the vocabulary, so the document would
    /// read as literal punctuation everywhere else; `<video>` is what every other
    /// renderer already understands, and what leaf's own reader picks back up
    /// through `html_elements` promotion (see [`parse_extensions`]).
    ///
    /// The one-line spelling needs twig ≥ 2.5.1, which widened CommonMark's
    /// HTML-block tag list to cover `<video>`/`<audio>`/`<picture>` under
    /// `html_elements`. Before that only the multi-line form parsed as a block at
    /// all, and this wrote three lines to work around it.
    ///
    /// `controls` is always written: a player with no transport is a still frame
    /// the reader can't do anything with. Any selection becomes the element's
    /// fallback text, exactly as it becomes an image's alt.
    ///
    /// The same verbatim-insertion caveat as [`insert_image`](Self::insert_image)
    /// applies, and bites harder here: a `"` in `destination` closes the
    /// attribute. A frontend taking these from a file picker is fine; one taking
    /// them from free text should keep them tame.
    ///
    /// [`MediaInfo`]: crate::MediaInfo
    pub fn insert_media(&mut self, kind: MediaKind, destination: &str, alt: &str) {
        if kind == MediaKind::Image {
            return self.insert_image(destination, alt);
        }
        // Gated on the *image* gesture, not on one of its own — there isn't one,
        // since the bytes below are spelled here rather than by twig, and an HTML
        // document would in fact parse them. The button is one control with three
        // kinds behind it, and two of them working in a format where the third
        // cannot is a worse surface than three that agree — especially as
        // `insert_image` is the kind anyone reaches for first.
        if self.refuse_unsupported("media", Gesture::InsertImage) {
            return;
        }
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        let alt_text = self
            .selected_text()
            .map(str::to_string)
            .unwrap_or_else(|| alt.to_string());
        let tag = match kind {
            MediaKind::Audio => "audio",
            _ => "video",
        };
        let markup = format!("<{tag} src=\"{destination}\" controls>{alt_text}</{tag}>");
        self.edit(start, end, &markup);
    }

    /// Append media at the **end** of the document, as a block of its own —
    /// the verb for a picture that *arrives* rather than one the writer
    /// places: an attachment imported while the caret was wherever it last
    /// was, a drawing placed from a tray under the body. At the caret it
    /// would land inline in front of whatever word the caret happened to be
    /// beside, which for an editor nobody has tapped yet is the first word
    /// of the document.
    ///
    /// The document is ended with a blank line first, where it does not
    /// already end with one, so the media is a paragraph of its own rather
    /// than a lazy continuation of the last one; an empty document needs no
    /// separator. Then [`insert_media`](Self::insert_media) at the new end,
    /// with everything that means: the same markup per kind, the same
    /// refusal on a format without images. The selection is dropped — the
    /// verb is about the end of the document, not about what was selected —
    /// and the caret is left past the media, as `insert_media` leaves it.
    pub fn append_media(&mut self, kind: MediaKind, destination: &str, alt: &str) {
        if self.read_only || self.refuse_unsupported("media", Gesture::InsertImage) {
            return;
        }
        let separator = if self.source.is_empty() || self.source.ends_with("\n\n") {
            ""
        } else if self.source.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let end = self.source.len();
        self.anchor = None;
        self.caret = end;
        if !separator.is_empty() {
            self.edit(end, end, separator);
        }
        self.anchor = None;
        self.caret = self.source.len();
        self.insert_media(kind, destination, alt);
    }

    /// Insert a thematic break at the caret — the toolbar's Horizontal Rule
    /// button. Spelling and placement are both twig's; leaf used to write `---`
    /// itself, which was the Markdown spelling in a djot document too.
    ///
    /// A rule is a block, so `insert_thematic_break` alone has nowhere to put one
    /// mid-paragraph and lands it after the caret's whole block. To get a rule
    /// *at* the caret — the paragraph parted in two around it, which is what a
    /// rule button is understood to do — the paragraph is first divided with
    /// `split_block` and the rule then aimed at the **first** half. Aiming it at
    /// the offset `split_block` returns puts the rule after the *second* half
    /// instead, which is a rule in the right document and the wrong place.
    ///
    /// Only a plain paragraph is split, and only where there is something to
    /// part: at the paragraph's end the split has no second half to mint and
    /// would write the separator anyway — a blank line and the empty slot Enter
    /// leaves for the next paragraph, which the rule then lands above and
    /// nothing fills — so there the rule goes straight after the paragraph,
    /// which is where the split-and-aim was sending it regardless. At the
    /// paragraph's *start* the split is kept, though it parts nothing either:
    /// `|para` becomes `\npara` with the caret on the new blank line, and a
    /// rule aimed at a blank line is written on it (twig ≥ 3.5.2), which is how
    /// "before the paragraph" is said through a gesture that only knows
    /// "after" — `---\n\npara`, and `prev\n\n---\n\npara` mid-document. Everywhere
    /// else the rule simply lands after the block, which is both twig's own
    /// answer and the better one: splitting a fenced code block would leave two
    /// fences with a rule between them, and splitting a list item would mint an
    /// item nobody asked for on the way to a rule that lands after the list
    /// regardless. A table and a setext heading refuse the split outright, so
    /// they take the same path by themselves.
    ///
    /// The caret ends on the line under the rule, which is where the writer
    /// goes on: at the start of the paragraph's second half when the rule
    /// parted one, and otherwise on an empty line opened under the rule — see
    /// [`stand_under_block`](Self::stand_under_block). It used to be left at
    /// the end of what twig wrote, which is the rule's own row: the caret was
    /// drawn beside the rule, and mid-document the next keystroke joined the
    /// block below.
    pub fn insert_thematic_break(&mut self) {
        if self.read_only || self.refuse_unsupported("thematic break", Gesture::InsertThematicBreak)
        {
            return;
        }
        self.caret = self.skip_trailing_close_delims(self.caret);
        // A selection is replaced by the rule, so collapse it first and let the
        // split-and-rule below run from the caret it leaves behind.
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
        }
        self.anchor = None;
        self.record_caret();
        let at = self.caret;
        // A failure here is not fatal: the rule still lands after the block,
        // which is exactly what this call was trying to improve on.
        let parted = self.part_for_block(at);
        match self.editor.insert_thematic_break(at) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                if parted {
                    self.coalesce_last_undo();
                }
                self.anchor = None;
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
                self.caret_under_written_block(change.new, parted);
            }
            Err(e) => self.status = Some(format!("thematic break: {e}")),
        }
    }

    /// Part the bare paragraph at `at` for a block gesture, where
    /// [`caret_parts_bare_paragraph`](Self::caret_parts_bare_paragraph) says
    /// to, and say whether it was parted.
    ///
    /// The split is counted as a step of its own, so the gesture can fold the
    /// block it writes next into it. It used to go to twig uncounted: twig
    /// kept it as a step and leaf's count did not, so one Undo took back the
    /// block and left the paragraph parted, with Undo then reporting nothing
    /// left to undo.
    pub(super) fn part_for_block(&mut self, at: usize) -> bool {
        if !self.caret_parts_bare_paragraph() || self.editor.split_block(at).is_err() {
            return false;
        }
        self.refresh();
        true
    }

    /// Where a block gesture leaves the caret once twig has written the block
    /// `new` spans — a rule, a page break: on the line under it. That is the
    /// start of the paragraph's second half when the gesture `parted` one,
    /// and otherwise the line [`stand_under_block`](Self::stand_under_block)
    /// finds or writes, folded into the gesture's step.
    ///
    /// The block is the outermost node twig's change opens, whatever its
    /// kind, since the kind a leaf directive comes back as is the format's.
    pub(super) fn caret_under_written_block(&mut self, new: Range<usize>, parted: bool) {
        let block = self
            .nodes()
            .into_iter()
            .filter(|n| n.kind != Kind::Doc && new.contains(&n.span.start))
            .min_by_key(|n| (n.span.start, std::cmp::Reverse(n.span.end)));
        let Some(block) = block else {
            return;
        };
        let (end, _) = wysiwyg::block_line(&self.source, block.span.end);
        if parted {
            // The second half starts at the first text under the block; the
            // split left it no leading whitespace.
            let rest = &self.source[end..];
            self.caret = end + (rest.len() - rest.trim_start().len());
            self.record_caret();
        } else if self.stand_under_block(end) {
            self.coalesce_last_undo();
        }
    }

    /// Stand the caret on the line under the block whose last line's text ends
    /// at `end` — a thematic break, a page break — first writing that line if
    /// the block has none: where the rule and page-break buttons leave the
    /// caret, and where Enter on a rule takes it. Whether anything was
    /// written, so a caller can fold it into its own step.
    ///
    /// The caret's line is the first under the block that the view gives a
    /// home: the second blank line in [`LineFlow::Fold`], which draws the
    /// first as the gap closing the block, and the first in
    /// [`LineFlow::Preserve`], where every blank line is somewhere to type.
    /// When anything follows, one more blank line has to stand between the
    /// caret's line and it, or what is typed there runs on into the next
    /// block. Only what that shape is missing is written. A block twig wrote on
    /// a blank line still has the blank lines that were under the caret and
    /// needs one line more, or none, where a rule written under a paragraph
    /// needs two.
    ///
    /// Blank means blank inside the block's container: a quote's blank lines
    /// keep their `>`, and the caret's line wears the quote's whole prefix, as
    /// the line Enter opens in a quote does. The document's last line, when no
    /// newline ends it, counts only once a blank line stands above it, since
    /// only then does the view draw it as a row. Fold always writes the gap
    /// above it if it isn't there, so there a block ending the document takes
    /// one newline more and the caret its end.
    pub(super) fn stand_under_block(&mut self, end: usize) -> bool {
        let prefix = self.continuation_prefix_at(end);
        let blank = prefix.trim_end();
        let gap = usize::from(self.line_flow == LineFlow::Fold);
        // The blank lines directly under the block's own, by where each starts,
        // and whether the line that ends the run has anything on it.
        let mut blanks = Vec::new();
        let mut follows = false;
        let mut at = end;
        while let Some(nl) = self.source[at..].find('\n') {
            let start = at + nl + 1;
            let len = self.source[start..].find('\n');
            let line = &self.source[start..len.map_or(self.source.len(), |len| start + len)];
            let line = line.trim_end();
            if line != blank {
                follows = !line.trim().is_empty();
                break;
            }
            let Some(len) = len else {
                if gap > 0 || !blanks.is_empty() {
                    blanks.push(start);
                }
                break;
            };
            blanks.push(start);
            at = start + len;
        }
        let missing = (gap + 1 + usize::from(follows)).saturating_sub(blanks.len());
        // What is missing goes in at the rule's end, above the blank lines
        // already there, so the caret's line is the same one counted from
        // the rule either way: written here, or one of those, moved down.
        let mut text = String::new();
        let mut caret = None;
        for line in 1..=missing {
            text.push('\n');
            if line == gap + 1 {
                text.push_str(&prefix);
                caret = Some(end + text.len());
            } else {
                text.push_str(blank);
            }
        }
        let caret = caret.unwrap_or_else(|| {
            let start = blanks[gap - missing];
            let line = self.source[start..].split('\n').next().unwrap_or("");
            start + text.len() + line.trim_end_matches('\r').len().min(prefix.len())
        });
        if !text.is_empty() && !self.splice(end, end, &text, EditKind::Other) {
            return false;
        }
        self.caret = caret.min(self.source.len());
        self.anchor = None;
        self.goal_col = None;
        self.record_caret();
        !text.is_empty()
    }

    /// The thematic break the caret stands under, as its text's end, when
    /// Enter there belongs to the rule: the caret at the rule's home past it
    /// (the start of the line under it), that line blank, and the caret drawn
    /// on the rule's own row.
    ///
    /// Drawn on the rule's row, because the home past a rule is also the start
    /// of the line under it, and which of the two the view draws it on is the
    /// flow's. [`LineFlow::Fold`] draws that line as the gap under the rule,
    /// no caret's home, so the caret is beside the rule. [`LineFlow::Preserve`]
    /// makes every blank line a home, so it draws the caret there and Enter is
    /// an ordinary blank line's — unless it is the document's unterminated
    /// last line, which is no row in either flow.
    pub(super) fn rule_above_caret(&mut self) -> Option<usize> {
        let caret = self.caret.min(self.source.len());
        let above = self.source[..caret].strip_suffix('\n')?;
        let above_start = above.rfind('\n').map_or(0, |i| i + 1);
        let text = above[above_start..].trim_end();
        let (last, _) = text.char_indices().next_back()?;
        let rule = self
            .editor
            .ancestors_at(above_start + last)
            .ok()?
            .into_iter()
            .find(|m| m.kind == Kind::ThematicBreak)?;
        let (end, home) = wysiwyg::block_line(&self.source, rule.span.end);
        if home != caret {
            return None;
        }
        let blank = self.continuation_prefix_at(end);
        let line_end = self.source[caret..].find('\n').map(|i| caret + i);
        let line = &self.source[caret..line_end.unwrap_or(self.source.len())];
        if line.trim_end() != blank.trim_end() && !line.trim().is_empty() {
            return None;
        }
        (self.line_flow == LineFlow::Fold || line_end.is_none()).then_some(end)
    }

    /// Insert a fresh table at the caret — the toolbar's Table button. One
    /// header row, `rows` empty body rows, `cols` columns, spelled by twig in
    /// the document's own dialect and placed the way its thematic break is:
    /// after the caret's block, blank-separated. A bare paragraph is parted
    /// around the caret first, exactly as
    /// [`insert_thematic_break`](Self::insert_thematic_break) parts it, so the
    /// table lands *at* the caret rather than after everything the caret's
    /// paragraph says.
    ///
    /// The caret ends in the first header cell, selected the way Tab selects
    /// a cell — the natural next act is to type the heading, and Tab then
    /// walks the grid. That cell is read back from the rebuilt table map
    /// rather than computed from the splice, because twig's blank line and
    /// quote prefix put the first bar at an offset only the reparse knows.
    ///
    /// The shape is the caller's: a menu offers a few, a dialog asks. Zero
    /// rows or columns is twig's refusal (a header with nothing under it is
    /// what its row delete refuses to leave), reported through `status` —
    /// and refused here before the paragraph is parted, which would otherwise
    /// be left parted around a table never written.
    pub fn insert_table(&mut self, rows: usize, cols: usize) {
        if self.read_only || self.refuse_unsupported("table", Gesture::InsertTable) {
            return;
        }
        // twig's one refusal of a shape, asked before the paragraph is parted
        // for a table that would never be written into it.
        if rows == 0 || cols == 0 {
            self.status = Some("table: needs at least one row and one column".into());
            return;
        }
        self.caret = self.skip_trailing_close_delims(self.caret);
        if let Some((s, e)) = self.selection() {
            self.splice_selection(s, e, "", EditKind::Other);
        }
        self.anchor = None;
        self.record_caret();
        let at = self.caret;
        let parted = self.part_for_block(at);
        match self.editor.insert_table(at, rows, cols) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                if parted {
                    self.coalesce_last_undo();
                }
                self.anchor = None;
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                // Into the first header cell of the table just written: the
                // first table whose grid begins inside the splice.
                self.rebuild_map();
                let first_cell = self
                    .vmap
                    .tables
                    .iter()
                    .filter_map(|t| t.grid.first().and_then(|row| row.cells.first()))
                    .find(|cell| cell.start >= change.new.start && cell.start < change.new.end)
                    .map(|cell| (cell.start, cell.end));
                if let Some((start, end)) = first_cell {
                    self.select_cell(start, end);
                }
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("table: {e}")),
        }
    }

    /// Whether the caret sits in a paragraph and nothing else — no list item, no
    /// quote, no fence, no table — with paragraph text still ahead of it. The
    /// one shape where parting the block around the caret is unambiguously what
    /// a rule button means; see
    /// [`insert_thematic_break`](Self::insert_thematic_break) for why every other
    /// container is left to take the rule after itself.
    ///
    /// The "text ahead" half is what keeps `split_block` from running at the
    /// one edge where its output composes badly. At a paragraph's end twig
    /// cannot mint the empty second half (no format spells an empty
    /// paragraph), so it writes only the separator — a blank line and the
    /// slot Enter leaves for the paragraph to come — and a block then aimed at
    /// the first half lands above a slot that nothing fills: `para\n` with the
    /// caret at 4 came out as `para\n\n* * *\n\n\n`. Trailing whitespace counts
    /// as nothing ahead, since the split would shed it as the second half's
    /// leading indent and leave the same slot. Which end of the newline a
    /// paragraph's span stops at differs between the formats (Markdown before
    /// it, djot after), which is why this reads the remaining bytes rather
    /// than comparing offsets. The paragraph's
    /// start is deliberately not the same case — see
    /// [`insert_thematic_break`](Self::insert_thematic_break) for why that
    /// split is kept.
    fn caret_parts_bare_paragraph(&mut self) -> bool {
        let caret = self.caret.min(self.source.len());
        let Ok(chain) = self.editor.ancestors_at(caret) else {
            return false;
        };
        let mut para_end = None;
        for m in chain {
            match m.kind {
                Kind::Para => para_end = Some(m.span.end.min(self.source.len())),
                Kind::ListItem
                | Kind::TaskListItem
                | Kind::BlockQuote
                | Kind::CodeBlock
                | Kind::Table => return false,
                _ => {}
            }
        }
        match para_end {
            Some(end) if end > caret => !self.source[caret..end].trim().is_empty(),
            _ => false,
        }
    }
}
