//! The single place source changes: a splice re-anchors the caret, repairs
//! mark edges, renumbers ordered lists, and keeps a setext heading from collapsing.

use super::*;

impl Doc {
    /// Replace a selection with `text`, taking the words it covers and none of
    /// the markup of the runs it only cuts.
    ///
    /// A selection is a range of what the rich view draws, but its ends are
    /// source offsets, and a run's delimiters are hidden between two of them.
    /// From the end of a bold word to the start of the next paragraph — the one
    /// stop a `"\n"` is, and what iOS hands `replace(_:withText:)` — the
    /// source is the run's closing `***` and the blank line, and splicing it
    /// left `***HENRY B EYRINGWhy`, the opening delimiters on screen with
    /// nothing to close them. A selection from inside a word to past it did the
    /// same on one line: `a **bold** text` lost `ld te` and kept `a **boxt`.
    ///
    /// So a run whose text survives keeps the delimiters that fall in the
    /// range, which leaves the new text where the selection began: inside a
    /// run cut at its end, which closes after the text's first line, and
    /// outside one cut at its start, which opens after all of it. A run whose text the range takes whole goes whole, as
    /// [`widen_over_emptied_inlines`](Self::widen_over_emptied_inlines) has a
    /// word-delete take it — unless the range lies inside that run, which is
    /// the mark-edge rule's case: it takes the delimiters and arms the marks,
    /// so text typed over a selected bold word is still bold.
    ///
    /// Source view edits the literal, and splices the range as it is.
    pub(super) fn splice_selection(
        &mut self,
        start: usize,
        end: usize,
        text: &str,
        kind: EditKind,
    ) -> bool {
        if self.view == View::Source || start >= end {
            return self.splice(start, end, text, kind);
        }
        let nodes: Vec<(Range<usize>, Range<usize>)> = self
            .nodes()
            .iter()
            .filter(|n| wysiwyg::is_inline(n))
            .filter_map(|n| Some((n.span.clone(), inline_content_span(n, &self.source)?)))
            .filter(|(span, text)| *span != *text)
            .collect();
        let (mut s, mut e) = (start, end);
        loop {
            let mut grew = false;
            for (span, body) in &nodes {
                let emptied = s <= body.start && body.end <= e;
                let inside = span.start <= s && e <= span.end;
                if emptied && !inside && (span.start < s || span.end > e) {
                    s = s.min(span.start);
                    e = e.max(span.end);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        // The delimiters in the range of the runs whose text survives: the
        // closing ones of a run cut at its end, the opening ones of a run cut
        // at its start. A run cut at both ends holds the whole range, so
        // neither of its delimiters is in it.
        let kept = |side: fn(&Range<usize>, &Range<usize>) -> Range<usize>| -> String {
            let mut kept: Vec<Range<usize>> = nodes
                .iter()
                .filter(|(_, body)| body.start < s || body.end > e)
                .map(|(span, body)| side(span, body))
                .map(|d| d.start.max(s)..d.end.min(e))
                .filter(|d| d.start < d.end)
                .collect();
            kept.sort_by_key(|d| d.start);
            kept.dedup();
            kept.into_iter().map(|d| &self.source[d]).collect()
        };
        let closes = kept(|span, body| body.end..span.end);
        let opens = kept(|span, body| span.start..body.start);
        if closes.is_empty() && opens.is_empty() {
            return self.splice(s, e, text, kind);
        }
        // A run closes on the line it opened on, so a run cut at its end
        // takes the text up to the first line break, less the whitespace a
        // closing delimiter cannot follow; a paste of several paragraphs puts
        // the rest after the run. A run cut at its start opens after all of
        // `text`, so the new text stays out of it.
        let line = text.find('\n').unwrap_or(text.len());
        let cut = text[..line].trim_end().len();
        let replacement = format!("{}{closes}{}{opens}", &text[..cut], &text[cut..]);
        if !self.splice(s, e, &replacement, kind) {
            return false;
        }
        // The caret after `text`: inside a run it ended in, before the
        // delimiters written behind it.
        let at = if cut == text.len() {
            s + text.len()
        } else {
            s + text.len() + closes.len()
        };
        if self.source.get(s..s + replacement.len()) == Some(replacement.as_str()) {
            self.caret = at;
            self.record_caret();
        }
        true
    }

    /// One splice of document text, keeping the **mark-edge rule**: an inline
    /// mark's content never begins or ends with whitespace. In Markdown and Djot
    /// a delimiter standing against a space is not a delimiter at all — `**bold **`
    /// is four literal asterisks around a word, and a rich view drawing the
    /// document faithfully has no choice but to show them. That is correct
    /// rendering of what the file says, and nobody typing a space after a bold
    /// word meant to say it.
    ///
    /// So the space goes *outside* the run instead — `**bold** ` — which is the
    /// same document to a reader and a live one to a parser. The caret follows it
    /// out and keeps the marks armed (see [`rearm`](Self::rearm)), so the next
    /// character rejoins the run (see [`rejoin_run`](Self::rejoin_run)) and the
    /// writer sees one unbroken bold phrase, never a flash of raw syntax.
    ///
    /// Every ordinary edit — typing, deleting, pasting, an IME step — comes
    /// through here, so the rule holds however the whitespace arrives at the
    /// edge. The repair is decided *after* the plain edit, by asking whether the
    /// mark actually died: a code span's backticks aren't whitespace-sensitive
    /// (`` `code ` `` is still code), and nothing is re-spelled when nothing broke.
    pub(super) fn splice(&mut self, start: usize, end: usize, text: &str, kind: EditKind) -> bool {
        let fix = self.mark_edge_fix(start, end, text);
        if !self.splice_exact(start, end, text, kind) {
            return false;
        }
        if let Some(fix) = fix {
            self.repair_mark_edges(fix);
        }
        if text.is_empty() && end > start {
            self.settle_inside_close_delims();
        }
        true
    }

    /// After a delete, take a caret left standing past a run's closing delimiters
    /// back inside the run.
    ///
    /// A delete leaves the caret where the deleted bytes began, and when those
    /// bytes were the last thing after a marked phrase — the space the mark-edge
    /// rule pushed out of `**bold** `, say — that spot is the far side of the
    /// closing `**`. The rich view has nothing to draw there: the delimiters are
    /// hidden, so the caret shows at the end of the word either way, and the two
    /// offsets are one place on screen with two different meanings. Typing at the
    /// outer one lands past the run, so the writer who backspaced a space out of
    /// their bold phrase watches the next character come out plain, and the
    /// toolbar button go dark, with the caret never appearing to move.
    ///
    /// The end of the run's text is the caret's home there — a delete that took
    /// away everything after a phrase leaves the caret at the end of that phrase,
    /// which is inside it — so it settles onto that
    /// ([`step_inside_close_delims`](Self::step_inside_close_delims) does the
    /// walk, through every mark closing at the point): the word stays bold, the
    /// button stays lit, and the next character carries on the phrase.
    ///
    /// Rich view only, and only where a mark really closes at the caret — mid-run
    /// or in plain prose no span ends there and the caret stays put. The opening
    /// edge is left alone on purpose: a caret in front of a run inherits from the
    /// text on its left, which is the plain text outside.
    fn settle_inside_close_delims(&mut self) {
        if self.view != View::Wysiwyg {
            return;
        }
        let at = self.step_inside_close_delims(self.caret);
        if at != self.caret {
            self.caret = at;
            self.clear_pending();
            self.record_caret();
        }
    }

    /// The splice exactly as asked, with no mark-edge repair — for the callers
    /// that are *writing* the delimiters themselves ([`insert_with_marks`](Self::insert_with_marks)
    /// and [`rejoin_run`](Self::rejoin_run)) and place their own offsets around
    /// the bytes they inserted.
    ///
    /// One `edit_range` through twig, then re-anchor the caret from the returned
    /// `Change` and refresh the cached source. A reparse-breaking edit (rare for
    /// Markdown/Djot) leaves the document untouched and reports.
    ///
    /// Returns whether the edit landed — for a caller that has offsets of its
    /// own to place afterwards, which a rolled-back splice would leave pointing
    /// into text that never came to exist.
    pub(super) fn splice_exact(
        &mut self,
        start: usize,
        end: usize,
        text: &str,
        kind: EditKind,
    ) -> bool {
        // The read-only gate, for every edit at once — see the field.
        if self.read_only {
            return false;
        }
        // twig records an undo step for every edit; when this one continues a
        // run of the same kind (typing, deleting), tell twig to fold it into the
        // step before it so the whole run undoes at once.
        let coalesce = kind != EditKind::Other && self.last_edit_kind == Some(kind);
        // Hand twig the pre-edit caret before the splice, so the undo step it
        // retires carries where the caret was standing.
        self.record_caret();
        match self.editor.edit_range(start, end, text) {
            Ok(change) => {
                self.last_edit_kind = Some(kind);
                // Counted first, so the fold has the step it folds.
                self.refresh();
                if coalesce {
                    self.coalesce_last_undo();
                }
                self.caret = change.new.end;
                self.anchor = None;
                self.goal_col = None;
                self.clear_pending();
                self.dirty = self.source != self.clean_source;
                self.status = None;
                // And the post-edit caret, so a later redo restores it.
                self.record_caret();
                true
            }
            // The edit was rolled back, so twig's history did not move and
            // neither may ours: pushing here would leave a step with no edit
            // under it and shift every later undo onto the wrong caret.
            Err(e) => {
                self.status = Some(format!("edit: {e}"));
                false
            }
        }
    }

    /// The re-spelling that would keep the mark-edge rule for the edit
    /// `[start, end)` → `text`, or `None` when the edit leaves no whitespace
    /// against a delimiter and the plain splice is already right. Computed
    /// *before* the edit, while the run's spans and delimiters can still be read
    /// off the document; applied afterwards, and only if the mark really died —
    /// see [`repair_mark_edges`](Self::repair_mark_edges).
    ///
    /// Rich view only. Source view is for typing raw markup, where a space put
    /// against a `**` is exactly the character it looks like.
    fn mark_edge_fix(&mut self, start: usize, end: usize, text: &str) -> Option<MarkEdgeFix> {
        if self.view != View::Wysiwyg || start > end || end > self.source.len() {
            return None;
        }
        // Every inline mark standing over the edit, outermost first, with the
        // content span that says where its delimiters are.
        let chain: Vec<(InlineKind, std::ops::Range<usize>, std::ops::Range<usize>)> = self
            .editor
            .ancestors_at(start)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                let kind = inline_kind(&m.kind)?;
                let content = m.content_span.clone()?;
                Some((kind, m.span.clone(), content))
            })
            .collect();
        // The innermost run whose *content* holds the whole edit: the one whose
        // text is being changed, rather than one the edit merely sits under.
        let (kind, span, content) = chain
            .iter()
            .rev()
            .find(|(_, _, c)| c.start <= start && end <= c.end)?
            .clone();
        // What that content becomes. Whitespace at either end of it is what
        // would put out the mark.
        let body = format!(
            "{}{text}{}",
            &self.source[content.start..start],
            &self.source[end..content.end]
        );
        let (lead, trail) = if body.trim().is_empty() {
            // Nothing but whitespace left: there is no content to mark at all,
            // and the delimiters go with it rather than closing on a space.
            (body.len(), 0)
        } else {
            (
                body.len() - body.trim_start().len(),
                body.len() - body.trim_end().len(),
            )
        };
        // Nothing against a delimiter, and something still between them: the
        // plain edit stands. An emptied run is broken just as surely (`**b**`
        // with the `b` deleted is the literal `****`) and is re-spelt as the
        // nothing it now says.
        if lead == 0 && trail == 0 && !body.is_empty() {
            return None;
        }
        // Marks that open or close exactly where this one does — `***both***` is
        // two runs sharing an edge — spell their delimiters as one run of bytes,
        // so the whitespace has to clear all of them together.
        let (mut open_at, mut close_at) = (span.start, span.end);
        for _ in 0..chain.len() {
            match chain.iter().find(|(_, _, c)| c.start == open_at) {
                Some((_, s, _)) => open_at = s.start,
                None => break,
            }
        }
        for _ in 0..chain.len() {
            match chain.iter().find(|(_, _, c)| c.end == close_at) {
                Some((_, s, _)) => close_at = s.end,
                None => break,
            }
        }
        let open = &self.source[open_at..content.start];
        let close = &self.source[content.end..close_at];
        let core = &body[lead..body.len() - trail];
        let respelt = if core.is_empty() {
            body.clone()
        } else {
            format!(
                "{}{open}{core}{close}{}",
                &body[..lead],
                &body[body.len() - trail..]
            )
        };
        // The caret sits just past the inserted text within the new content —
        // which, when that lands in the whitespace, is now outside the delimiters.
        let pos = (start - content.start) + text.len();
        let caret = if core.is_empty() || pos <= lead {
            open_at + pos
        } else if pos >= lead + core.len() {
            open_at + lead + open.len() + core.len() + close.len() + (pos - lead - core.len())
        } else {
            open_at + lead + open.len() + (pos - lead)
        };
        Some(MarkEdgeFix {
            kind,
            probe: content.start,
            start: open_at,
            end: close_at + text.len() - (end - start),
            text: respelt,
            caret,
            // The marks in force here, resolved against any armed sticky delta —
            // what the writer is typing in, and so what has to still be true on
            // the far side of the delimiter the caret just stepped over.
            want: chain
                .iter()
                .filter(|(_, s, _)| start < s.end)
                .map(|(k, _, _)| *k)
                .collect::<InlineMarks>()
                .xor(self.pending_here()),
        })
    }

    /// Apply a [`MarkEdgeFix`] — but only if the edit it was computed for really
    /// did break the mark. Whether whitespace at a delimiter is fatal is the
    /// format's business, not leaf's: `**bold **` is no longer strong, while
    /// `` `code ` `` is still perfectly good verbatim, and Djot's braced spellings
    /// don't care either. Asking the parser afterwards settles it for every kind
    /// and format at once, and costs a re-spelling only where one is due.
    ///
    /// The repair rides along with the edit that caused it — one undo step puts
    /// back what the writer typed, not a delimiter shuffle they never saw.
    fn repair_mark_edges(&mut self, fix: MarkEdgeFix) {
        if fix.end > self.source.len() {
            return;
        }
        if self.marks_at(fix.probe).iter().any(|(k, _)| *k == fix.kind) {
            return; // still a mark: these delimiters don't mind the whitespace
        }
        let resumed = self.last_edit_kind;
        if !self.splice_exact(fix.start, fix.end, &fix.text, EditKind::Other) {
            return;
        }
        self.coalesce_last_undo();
        // The keystroke owns the undo step, so the run of typing it belongs to
        // keeps coalescing over the repair rather than breaking in two here.
        self.last_edit_kind = resumed;
        self.caret = fix.caret.min(self.source.len());
        self.anchor = None;
        self.goal_col = None;
        self.rearm(fix.want);
        self.clamp_caret();
        self.record_caret();
    }

    /// Arm whatever sticky delta reproduces `want` at the caret — the marks the
    /// writer is typing in, carried across an edit that moved the caret out of
    /// the run holding them. Arms nothing when the caret already stands in
    /// exactly those marks, but still remembers the spot, so a further ⌘b starts
    /// a clean delta here (see [`toggle`](Self::toggle)).
    pub(super) fn rearm(&mut self, want: InlineMarks) {
        let here: InlineMarks = self
            .marks_at(self.caret)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        self.pending_marks = want.xor(here);
        self.pending_at = Some(self.caret);
    }

    /// Insert `text` at `at` as a *literal* run via twig's `insert_literal`,
    /// which backslash-escapes any character that would otherwise open markup in
    /// this format and position (`*` → `\*`, a line-start `#` → `\#`). The mirror
    /// of [`splice`](Self::splice) for the Hidden reveal mode's typing path, with
    /// the same caret re-anchor, coalescing, and rollback contract. `at` must be
    /// a collapsed point — a selection is deleted by the caller first, since
    /// `insert_literal` inserts rather than replaces.
    pub(super) fn insert_literal_at(
        &mut self,
        at: usize,
        text: &str,
        kind: EditKind,
        force_coalesce: bool,
    ) -> bool {
        // The read-only gate: this door goes to twig directly, not through
        // `splice_exact`, so it guards itself — see the field.
        if self.read_only {
            return false;
        }
        // `force_coalesce` folds this into the immediately preceding edit (the
        // selection-delete of an overwrite) so the pair is one undo step; else it
        // coalesces only when it continues a run of the same-kind typing.
        let coalesce =
            force_coalesce || (kind != EditKind::Other && self.last_edit_kind == Some(kind));
        // The mark-edge rule holds for typed text however it is spelled — see
        // `splice`. Only an insert twig passed through unchanged can use it,
        // since a fix is measured in the bytes that actually land, and an escape
        // adds bytes this couldn't have counted.
        let fix = self.mark_edge_fix(at, at, text);
        #[cfg(test)]
        if std::mem::take(&mut self.refuse_next_literal) {
            self.status = Some("edit: refused".into());
            return false;
        }
        self.record_caret();
        match self.editor.insert_literal(at, text) {
            Ok(change) => {
                self.last_edit_kind = Some(kind);
                // Counted first, so the fold has the step it folds.
                self.refresh();
                if coalesce {
                    self.coalesce_last_undo();
                }
                self.caret = change.new.end;
                self.anchor = None;
                self.goal_col = None;
                self.clear_pending();
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
                if let Some(fix) = fix.filter(|_| change.new.end - change.new.start == text.len()) {
                    self.repair_mark_edges(fix);
                }
                true
            }
            Err(e) => {
                self.status = Some(format!("edit: {e}"));
                false
            }
        }
    }

    /// After a structural list edit (a new item, a nest/unnest), renumber the
    /// ordered list the caret sits in so its source markers run `1, 2, 3, …`
    /// again — a raw splice leaves them stale (`1. 2. 2. 3.`). twig does the
    /// renumber as its own edit; fold it into the edit that triggered it so the
    /// two undo as one, and only when it actually changed the source (a no-op or
    /// a caret outside any ordered list must not coalesce the real edit into the
    /// step before it).
    pub(super) fn renumber_here(&mut self) {
        self.renumber_at(self.caret);
    }

    /// [`renumber_here`](Self::renumber_here) aimed somewhere other than the
    /// caret — for an edit that leaves the caret one past the item it just wrote,
    /// where twig resolves no list to renumber.
    pub(super) fn renumber_at(&mut self, off: usize) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        let before = self.source.clone();
        if self.editor.renumber_ordered_lists(off).is_err() {
            return; // not inside an ordered list — nothing to renumber
        }
        self.refresh();
        if self.source != before {
            self.coalesce_last_undo();
            self.dirty = self.source != self.clean_source;
            self.clamp_caret();
            self.record_caret();
        }
    }

    /// Repair the one trap a list edit can spring on itself. An *empty* `-`
    /// sub-item written directly beneath a text line reparses that text as a
    /// setext heading — `- hello\n  - ` is `<h2>hello</h2>`, because a lone `-`
    /// is also a setext-H2 underline (twig is right; pandoc agrees). `*` and `+`
    /// bullets can't underline anything, so swap the dash for a `*`: the item
    /// stays an empty nested bullet, the parent stays prose, and the source
    /// round-trips instead of hiding a heading the user never asked for. Folded
    /// into the triggering edit's undo step, the way renumbering is.
    ///
    /// Gated on the collapse having actually happened (the swapped dash was
    /// swallowed into a `heading`), so a real setext heading the author wrote —
    /// or a `- x` with content, which can't underline anything — is never
    /// touched. This has to live in the *edit*, not the renderer: leaving the
    /// hazardous bytes on disk and only painting over them would ship a file
    /// every other CommonMark tool reads as a heading.
    ///
    /// This one keeps its own byte scan, and has to: the hazard is precisely
    /// that the dash stopped being a list marker, so [`list_marker_on_line`] —
    /// which asks twig which lines open an item — reports nothing here. There is
    /// no node to ask about. It is also the last Markdown spelling leaf writes on
    /// purpose rather than for want of an answer; once twig spells continuations
    /// itself, avoiding the trap becomes twig's, and this goes.
    ///
    /// [`list_marker_on_line`]: Self::list_marker_on_line
    pub(super) fn avoid_setext_collapse(&mut self) {
        let caret = self.caret.min(self.source.len());
        let line_start = self.source[..caret].rfind('\n').map_or(0, |i| i + 1);
        let bytes = self.source.as_bytes();
        let mut dash = line_start;
        while matches!(bytes.get(dash), Some(b' ' | b'\t')) {
            dash += 1;
        }
        // A dash bullet is the only marker that doubles as a setext underline.
        if bytes.get(dash) != Some(&b'-') {
            return;
        }
        // Only an *empty* item is a bare underline; `- x` carries content and
        // can't fold the line above into a heading.
        let line_end = self.source[dash..]
            .find('\n')
            .map_or(self.source.len(), |i| dash + i);
        if !self.source[dash + 1..line_end].trim().is_empty() {
            return;
        }
        // The tell: that dash was swallowed into a `heading`. A properly nested
        // empty item sits under a `list_item`, with no heading in reach. Probe
        // the dash byte itself (well inside the heading), not the caret, whose
        // end-of-line offset can fall on the half-open span boundary.
        let collapsed = self
            .editor
            .ancestors_at(dash)
            .map(|c| c.into_iter().any(|m| m.kind == Kind::Heading))
            .unwrap_or(false);
        if !collapsed {
            return;
        }
        let caret = self.caret;
        if self.splice(dash, dash + 1, "*", EditKind::Other) {
            // Same width, so the caret keeps its column; fold into the edit that
            // triggered this so Tab stays one undo step.
            self.coalesce_last_undo();
            self.caret = caret.min(self.source.len());
            self.clamp_caret();
            self.record_caret();
        }
    }
}
