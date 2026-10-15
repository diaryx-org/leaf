//! Inline marks: toggling `**bold**` and friends, and a highlight's colour.

use super::*;

impl Doc {
    fn snapshot(&self) -> CaretState {
        CaretState {
            caret: self.caret,
            anchor: self.anchor,
        }
    }

    /// Hand twig the current caret and selection as the blob for the live
    /// document state. Called before an edit — so the step twig retires records
    /// where the caret was, and undo can restore it — and again once the op has
    /// placed the caret, so redo restores where the edit left it.
    ///
    /// This is the whole of leaf's undo-caret bookkeeping now. twig carries the
    /// caret through its own history, so coalescing falls out for free (folding
    /// two twig steps into one drops the intermediate blob, keeping the run's
    /// first) and the parallel stacks that had to march in lockstep — and could
    /// silently drift out of it — are gone.
    pub(super) fn record_caret(&mut self) {
        let _ = self.editor.set_caret_blob(&self.snapshot().to_blob());
    }

    /// Toggle an inline mark over the selection (Bold / Italic / Code / …). Keeps
    /// the toggled region selected so a second press cleanly reverses it.
    pub fn toggle(&mut self, kind: InlineKind) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        // Ahead of the no-selection branch below: arming a mark for text not yet
        // typed is a promise `insert` cannot keep in a format with no delimiters
        // to spell it with. Per *kind*, not per format — Markdown spells five
        // of the eight marks (highlight among them, under the `highlight`
        // extension leaf parses with), djot all eight, HTML seven.
        if self.refuse_unsupported(&format!("{kind:?}"), Gesture::ToggleInline(kind)) {
            return;
        }
        let Some((s, e)) = self.selection() else {
            // No selection: arm the mark for the next text typed here, the way a
            // word processor does. `⌘b`, type, `⌘b` again toggles bold on and off
            // in the flow of typing without ever selecting anything — the delta
            // is realised onto the freshly typed text by `insert`. A fresh caret
            // position starts the delta over from the marks actually in force.
            if self.pending_at != Some(self.caret) {
                self.pending_marks = InlineMarks::empty();
                self.pending_at = Some(self.caret);
            }
            self.pending_marks.flip(kind);
            self.status = None;
            return;
        };
        // Whitespace at the edge of a selection is not part of what was chosen —
        // a double-click takes the space after the word with it — and a mark
        // cannot close against one anyway: `**word **` is four literal asterisks
        // (the mark-edge rule, see `splice`). Mark the words, leave the spaces.
        let picked = &self.source[s..e];
        let (s, e) = (
            s + (picked.len() - picked.trim_start().len()),
            e - (picked.len() - picked.trim_end().len()),
        );
        if s >= e {
            self.status = Some(format!("{kind:?}: nothing selected to mark"));
            return;
        }
        // Styling a selection is a one-shot act, not a sticky mode.
        self.clear_pending();
        self.record_caret();
        match self.editor.toggle_inline(s, e, kind) {
            Ok(change) => {
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                self.anchor = Some(change.new.start);
                self.caret = change.new.end;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("{kind:?}: {e}")),
        }
    }

    /// Whether the caret stands in a highlight — what a frontend asks to enable
    /// or disable its highlight-colour controls, the way
    /// [`caret_in_table`](Self::caret_in_table) gates the grid ones.
    ///
    /// A fact about the *caret*, and the other half of
    /// [`Capabilities::mark_color`], which is the fact about the format. A
    /// frontend needs both: djot spells a highlight and no colour for it, so a
    /// caret standing in `{=word=}` answers `true` here and still has no palette
    /// to offer.
    ///
    /// The rule is [`active_inline_marks`](Self::active_inline_marks)' rule, so
    /// the palette appears exactly where the Highlight button is lit — with one
    /// deliberate exception: a mark *armed* at a bare caret and not yet typed
    /// into lights the button and answers `false` here, because there is no node
    /// to colour until the text exists.
    pub fn caret_in_mark(&mut self) -> bool {
        self.mark_offset().is_some()
    }

    /// The offset [`set_mark_color`](Self::set_mark_color) speaks for — the one
    /// standing in the highlight the gesture means — or `None` when neither end
    /// of what is selected is in one.
    ///
    /// The caret first, and the selection's *start* after it, because of what
    /// [`toggle`](Self::toggle) leaves behind: a fresh `==word==` is selected
    /// whole, with the caret at its far edge, one past the closing `==` and so
    /// (by `marks_at`' half-open rule) not in the mark at all. Highlight a word
    /// and colour it — the two presses a coloured highlight is made of — would
    /// otherwise refuse on the second, having just written the highlight the
    /// author is pointing at.
    fn mark_offset(&mut self) -> Option<usize> {
        let in_mark = |d: &mut Self, off: usize| {
            d.marks_at(off)
                .into_iter()
                .any(|(k, _)| k == InlineKind::Mark)
                .then_some(off)
        };
        let caret = self.caret.min(self.source.len());
        in_mark(self, caret).or_else(|| {
            let start = self.selection()?.0;
            in_mark(self, start)
        })
    }

    /// The colour of the highlight at the caret — `None` both when the caret is
    /// in no highlight and when the highlight it is in names no colour, which
    /// are the same answer to "which swatch is lit".
    ///
    /// The innermost mark, by span, for the same reason
    /// [`current_heading_level`](Self::current_heading_level) walks the tree:
    /// what the caret is *in* is the deepest node containing it. A `data-color`
    /// naming a colour this build has no variant for reads as `None` — the
    /// renderer already draws that as a plain highlight rather than guessing,
    /// and the toolbar agrees with the renderer.
    pub fn mark_color_at_caret(&mut self) -> Option<MarkColor> {
        let at = self.mark_offset()?;
        self.mark_color_at(at)
    }

    /// [`mark_color_at_caret`](Self::mark_color_at_caret) at a given offset —
    /// the innermost `mark` covering it, and the colour it names.
    fn mark_color_at(&mut self, off: usize) -> Option<MarkColor> {
        self.nodes()
            .into_iter()
            .filter(|n| n.kind == Kind::Mark)
            .filter(|n| n.span.start <= off && off < n.span.end)
            .min_by_key(|n| n.span.end - n.span.start)
            .and_then(|n| MarkColor::from_attrs(&n.attrs))
    }

    /// Colour the highlight at the caret, or clear its colour with `None` — the
    /// palette behind a toolbar's Highlight button.
    ///
    /// Markdown only, and the one gesture whose availability is a fact about the
    /// *parse extensions* rather than about the format alone: the colour is
    /// spelled `==🔴 text==`, an emoji twig reads back out of the content and
    /// records as the mark's `data-color`, and only an editor parsing with
    /// `highlight_colors` (which [`parse_extensions`] turns on for every leaf
    /// document) reads it back that way. Djot spells the highlight and no colour
    /// for it, so this refuses there — see [`Capabilities::mark_color`].
    ///
    /// **A colour is a property of a highlight that already exists.** There is
    /// no "highlight this in red" here, because that is two splices and would be
    /// two undo steps under one press; a frontend that wants it calls
    /// [`toggle`](Self::toggle) with [`InlineKind::Mark`] first, which is the
    /// order the two buttons already sit in. With no highlight at the caret this
    /// says so in the status line and writes nothing.
    ///
    /// The caret keeps its place in the *text*: the splice is entirely in the
    /// prefix between the opening `==` and the first word, so an offset past it
    /// rides the emoji's width, and one standing on the prefix itself lands
    /// where the prefix now ends.
    pub fn set_mark_color(&mut self, color: Option<MarkColor>) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return;
        }
        if self.refuse_unsupported("highlight colour", Gesture::SetMarkColor) {
            return;
        }
        let Some(at) = self.mark_offset() else {
            self.status = Some("highlight colour: no highlight at the caret".into());
            return;
        };
        // Clearing a colour a highlight hasn't got is twig's one *successful*
        // no-op, and the `Change` it hands back then describes whatever edit came
        // before it — a stale span that would drag the caret somewhere it never
        // was. Answer it here, where the question is cheap, rather than trusting
        // a change that isn't one.
        if color.is_none() && self.mark_color_at(at).is_none() {
            self.status = None;
            return;
        }
        self.record_caret();
        match self.editor.set_mark_color(at, color.map(twig_mark_color)) {
            Ok(change) => {
                // Re-anchored from the offsets as they were, *before* `refresh`
                // sees the new bytes: the caret it clamps is one standing inside
                // a prefix that didn't exist a moment ago, and walking it back to
                // a char boundary of the emoji loses the place this is restoring.
                let caret = reanchor(self.caret, &change);
                let anchor = self.anchor.map(|a| reanchor(a, &change));
                self.last_edit_kind = None; // structural edit is its own undo step
                self.refresh();
                self.caret = caret;
                self.anchor = anchor;
                self.dirty = self.source != self.clean_source;
                self.status = None;
                self.clamp_caret();
                self.record_caret();
            }
            Err(e) => self.status = Some(format!("highlight colour: {e}")),
        }
    }

    /// One press of a colour swatch: colour the highlight at the caret, or —
    /// over a selection that isn't highlighted yet — highlight it and colour it,
    /// as **one** undo step.
    ///
    /// [`set_mark_color`](Self::set_mark_color) is the exact gesture and stays
    /// one splice; this is the compound every toolbar actually presses, and it
    /// lives here rather than in each frontend because the rule it encodes —
    /// what a swatch means when there is no highlight under it yet — is one
    /// answer, not one per frontend. The two splices are folded into a single
    /// history step, so the press that made a red highlight is taken back by a
    /// single undo rather than leaving an uncoloured one behind.
    ///
    /// `None` clears the colour, and over an unhighlighted selection means
    /// simply "highlight this" — the same thing the Highlight button does.
    /// A bare caret in no highlight is left alone with a status line, because
    /// [`toggle`](Self::toggle) there arms a mark for text not yet typed and a
    /// colour cannot be armed with it.
    pub fn highlight(&mut self, color: Option<MarkColor>) {
        if self.caret_in_mark() || self.selection().is_none() {
            self.set_mark_color(color);
            return;
        }
        self.toggle(InlineKind::Mark);
        // The format may not spell a highlight at all (`toggle` said so), and
        // there is nothing to colour if it doesn't.
        if self.status.is_some() {
            return;
        }
        let before = self.revision;
        self.set_mark_color(color);
        // Only fold when the colour really spliced. `highlight(None)` over a
        // fresh highlight is a no-op by design, and coalescing there would eat
        // the *previous* edit into the toggle instead.
        if self.revision != before {
            self.coalesce_last_undo();
        }
    }
}
