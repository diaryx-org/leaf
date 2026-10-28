//! Typing: every insert and replacement funnels through here, along with the
//! pending inline marks a typed character picks up and IME composition.

use super::*;

impl Doc {
    // ── editing ──────────────────────────────────────────────────────────────

    /// Replace the byte range `[start, end)` with `text`, re-anchoring the caret
    /// after it. The public form of the internal splice — a pixel frontend that
    /// hit-tests to a byte offset (or an IME that hands back an explicit range)
    /// edits through this, the same twig `edit_range` the caret ops use.
    pub fn edit(&mut self, start: usize, end: usize, text: &str) {
        self.splice(start, end, text, EditKind::Other);
    }

    /// Replace `[start, end)` with `text` in place, behind the caret — an
    /// automatic substitution a host makes as the user types: a misspelling
    /// corrected, a text replacement expanded, a straight quote curled, a `--`
    /// made a dash. Whether it did: `false` for a read-only document, a range
    /// that is not one, or an edit twig refused.
    ///
    /// Unlike [`edit`](Self::edit), the caret and the selection stay where they
    /// were, moved along by the edit — a word corrected behind the caret does
    /// not pull the caret back to it — and so does a sticky mark armed at the
    /// caret. It is an undo step of its own, folded into neither the typing
    /// before it nor the typing after, so one undo puts back exactly what was
    /// typed, with the caret where it stood. The bytes are replaced exactly,
    /// snapping neither end, so a word at the edge of `**bold**` keeps its
    /// delimiters. `text` is written the way typing writes it: literally, with
    /// anything that would open markup escaped, where typing is literal
    /// ([`MarkupMode::None`] in the rendered view).
    pub fn substitute(&mut self, start: usize, end: usize, text: &str) -> bool {
        if self.read_only
            || start > end
            || end > self.source.len()
            || !self.source.is_char_boundary(start)
            || !self.source.is_char_boundary(end)
        {
            return false;
        }
        let before = self.source.len();
        let (caret, anchor) = (self.caret, self.anchor);
        let (pending_marks, pending_at) = (self.pending_marks, self.pending_at);
        let group_had_step = self.undo_group.map(|g| g.has_step);
        self.last_edit_kind = None;
        let literal = !self.markup_mode.authors()
            && self.view == View::Wysiwyg
            && !text.is_empty()
            && self.supports(Gesture::InsertLiteral);
        let landed = if literal {
            let deleted = self.source[start..end].to_owned();
            if start != end && !self.splice_exact(start, end, "", EditKind::Other) {
                false
            } else if self.insert_literal_at(start, text, EditKind::Other, start != end) {
                true
            } else {
                // The deletion landed and the text did not: take the deletion
                // back rather than leave half a substitution.
                if start != end {
                    self.restore_deleted(start, &deleted, group_had_step);
                }
                false
            }
        } else {
            self.splice_exact(start, end, text, EditKind::Other)
        };
        if !landed {
            self.caret = caret;
            self.anchor = anchor;
            self.record_caret();
            return false;
        }
        self.carry_selection(
            caret,
            anchor,
            (pending_marks, pending_at),
            start,
            end,
            before,
        );
        true
    }

    /// Bring the source to `text` in one step that leaves the caret where it
    /// was — what a host does when the file changed under an open editor, as
    /// another device's edit arriving does.
    ///
    /// Only what differs is replaced: twig's diff of the two texts
    /// ([`twig::Op::diff`]), applied as one edit. A caret or selection
    /// outside every change stays on the characters it was on, even between
    /// two changes far apart, and one inside a change goes to the end of what
    /// replaced it, as [`substitute`](Self::substitute) moves them. Unlike a
    /// substitution, `text` is source and written exactly — nothing in it is
    /// escaped — since it is the document as it now stands, not something
    /// typed. An undo step of its own. `true` with nothing done when the
    /// source is already `text`; `false` when the document is read-only or
    /// the edit was refused.
    pub fn replace_source(&mut self, text: &str) -> bool {
        if self.source == text {
            return true;
        }
        if self.read_only {
            return false;
        }
        let ops = Op::diff(&self.source, text);
        self.apply_ops(&ops)
    }

    /// Put the caret and selection back after an edit of `[start, end)` that
    /// left them wherever the edit did, moved along by it — see
    /// [`substitute`](Self::substitute). `before` is the source's length
    /// before the edit; `pending` the sticky marks armed at the old caret.
    fn carry_selection(
        &mut self,
        caret: usize,
        anchor: Option<usize>,
        pending: (InlineMarks, Option<usize>),
        start: usize,
        end: usize,
        before: usize,
    ) {
        let (pending_marks, pending_at) = pending;
        let delta = self.source.len() as isize - before as isize;
        let new_end = (end as isize + delta).max(start as isize) as usize;
        // A place after the range moves with it, one inside it goes to the end
        // of what replaced it, and one before it stays.
        fn shift(p: usize, start: usize, end: usize, new_end: usize, delta: isize) -> usize {
            if p >= end {
                (p as isize + delta).max(0) as usize
            } else if p > start {
                new_end
            } else {
                p
            }
        }
        self.caret = shift(caret, start, end, new_end, delta);
        self.anchor = anchor
            .map(|a| shift(a, start, end, new_end, delta))
            .filter(|&a| a != self.caret);
        if pending_at == Some(caret) {
            self.pending_marks = pending_marks;
            self.pending_at = Some(self.caret);
        }
        self.goal_col = None;
        self.last_edit_kind = None;
        self.clamp_caret();
        // The caret the step leaves, so a redo puts it back here too.
        self.record_caret();
    }

    /// Put back `deleted`, the bytes a half-made [`substitute`](Self::substitute)
    /// took out at `at`, and leave nothing of the pair on the history.
    ///
    /// Not [`undo`](Self::undo): that closes an open group and takes the whole
    /// of it back — every substitution of the keystroke made before this one
    /// — and leaves a redo step that would delete the bytes again. Instead the
    /// bytes go back as an edit of their own, and the two edits, which change
    /// nothing together, are folded away: inside a group that already had a
    /// step they fold into it as they land; as a group's first step, or
    /// outside a group, into the step before. With no step before it there is
    /// nothing to fold into, and one step that changes nothing is left.
    /// `group_had_step` is what the group said before the deletion — `None`
    /// outside one.
    fn restore_deleted(&mut self, at: usize, deleted: &str, group_had_step: Option<bool>) {
        if !self.splice_exact(at, at, deleted, EditKind::Other) {
            // twig will not take its own bytes back: the history's way, which
            // at least leaves the document as it was.
            self.undo();
            return;
        }
        match group_had_step {
            Some(true) => {}
            Some(false) => {
                let steps = self.undo_steps;
                self.fold_last_undo();
                if self.undo_steps < steps
                    && let Some(g) = &mut self.undo_group
                {
                    g.has_step = false;
                }
            }
            None => {
                self.fold_last_undo();
                self.fold_last_undo();
            }
        }
        self.last_edit_kind = None;
    }

    /// Insert typed `text` at the caret, replacing the selection if there is one.
    /// A single typed character coalesces with the run of typing before it; a
    /// newline or a multi-character insert is its own undo step.
    ///
    /// Typed input only — clipboard text goes through [`paste`](Self::paste).
    pub fn insert(&mut self, text: &str) {
        // The read-only gate, up front: the paths below reach twig by several
        // verbs, not all of them through the splice — see the field.
        if self.read_only {
            return;
        }
        // Typing against a block picture would dissolve it, and typing past a
        // table would grow it a row — see `open_paragraph_at_block_edge`. Give
        // the text a paragraph first, so what the caret was standing beside
        // stays what it was.
        self.open_paragraph_at_block_edge(text);
        // Armed sticky marks (⌘b with no selection) turn the next typed text
        // bold/italic/… and then retire — see `insert_with_marks`. Whitespace is
        // the exception: it takes no mark of its own and keeps the delta armed
        // for the character behind it — see `insert_space_with_marks`.
        let pending = self.pending_here();
        if !pending.is_empty() && self.selection().is_none() && !text.is_empty() {
            if text.trim().is_empty() {
                self.insert_space_with_marks(self.caret, text, pending);
            } else {
                self.insert_with_marks(self.caret, text, pending);
            }
            return;
        }
        // `MarkupMode::None`: typed syntax stays literal — twig escapes
        // anything that would open markup, so a Diaryx user never mints
        // formatting by keyboard (it comes from commands instead). The other two
        // rungs of the ladder author markup from what you type, which is the
        // whole difference between them and this one. Only in the rendered view
        // (source view is for typing raw markup) and only where the format has a
        // literal spelling at all: escaping is a backslash before a byte from the
        // format's own alphabet, and a format with no such alphabet (HTML escapes
        // with entities, XML spells nothing) would have `\&` written into it,
        // which is two literal characters and not an escape. Marks (⌘b) still
        // format — that path returned above; and leaf's own structural inserts go
        // through `insert_raw`, never here, so a list marker or quote gutter is
        // written as the markup it is.
        if !self.markup_mode.authors()
            && self.view == View::Wysiwyg
            && !text.is_empty()
            && self.supports(Gesture::InsertLiteral)
        {
            self.insert_literal_typed(text);
            return;
        }
        self.insert_raw(text);
    }

    /// Insert `text` verbatim at the caret (replacing any selection) — the plain
    /// path with no Hidden-mode literal escaping. leaf's own structural inserts
    /// (a list marker, a quote gutter, an in-cell `<br>`) call this: they ARE
    /// markup by design and must not be escaped.
    pub(super) fn insert_raw(&mut self, text: &str) {
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        self.splice_selection(s, e, text, typed_edit_kind(text));
    }

    /// Open a paragraph for text about to be inserted at one of a block media's
    /// two caret stops, or at a table's trailing stop, and leave the caret
    /// standing in it.
    ///
    /// A block image is a paragraph whose entire content is the picture, and the
    /// caret's only homes on it are in front of it and just past it (see
    /// [`VisualMap::block_media_stop`]). Text inserted at either offset joins
    /// *that* paragraph — and a paragraph holding anything besides the image is
    /// no longer a block image but a line of text with an inline one in it. The
    /// frontend that was painting a photo there paints a text run instead; the
    /// picture is still in the file, and nothing said a word. Those two offsets
    /// are also exactly where a click on the picture lands, so the whole accident
    /// is one tap and one keystroke.
    ///
    /// So the break goes in first and the text lands in the new empty paragraph —
    /// what pressing Return before typing would have done, which is a habit no
    /// one should have to learn from losing a photo. A no-op everywhere else, and
    /// over a selection (which is replaced, not joined into).
    ///
    /// A picture inside a quote or a list leaves its container, because `\n\n`
    /// ends the block. The alternative is worse: the `\n> ` / next-item
    /// continuation [`newline`](Self::newline) writes stays in the same
    /// *paragraph*, which is the thing being prevented.
    ///
    /// A table's trailing stop ([`VisualMap::table_end_stop`]) is the same
    /// accident from the other side of a different block: the stop sits at the
    /// end of the table's last source line, and a line glued under a table is
    /// a row of it — `| 1 | 2 |x` is a three-cell row, not a paragraph. So the
    /// break goes in there too, and the text lands under the table.
    ///
    /// A block leaf directive (`::embed{src=x}`, `::page-break`, djot's empty
    /// `::: name` fence) is drawn on the picture's recipe and dissolves the
    /// same way — `X::embed{src=x}` and `::embed{src=x}X` are paragraphs of
    /// raw source — so its two stops ([`VisualMap::block_directive_stop`])
    /// open a paragraph too. Two directives in a row with nothing between
    /// them get one between them, from either side.
    ///
    /// Only in the rendered view. Source view is for typing raw markup, where
    /// putting a character against an image is exactly what it looks like.
    fn open_paragraph_at_block_edge(&mut self, text: &str) {
        if self.view != View::Wysiwyg || text.is_empty() || text == "\n" {
            return;
        }
        if self.selection().is_some() {
            return;
        }
        // Only beside a block picture, a directive or a table is there an edge
        // to open a paragraph at. The walk to the caret says whether one is
        // there, which a map a revision behind would have to be rebuilt to
        // say: a frontend that draws no frame between keystrokes (Leaf's
        // pages) types a run of letters without building it once.
        let beside = self.nodes_at(self.caret).iter().any(|n| {
            matches!(
                n.kind,
                Kind::Image | Kind::DisplayMath | Kind::Table | Kind::Container | Kind::Other(_)
            )
        });
        if !beside {
            return;
        }
        // The map may be a revision behind (nothing has drawn since the last
        // edit), and this asks it about offsets — a stale answer would splice a
        // break into the wrong place. Free when it is already current, which it
        // is whenever a frontend drew a frame between keystrokes.
        self.rebuild_map();
        let at = self.caret;
        let (side, span) = match self.block_atom_stop(at) {
            Some(stop) => stop,
            None if self.vmap.table_end_stop(at) => (MediaStop::After, at..at),
            None => return,
        };
        // In front of the block, the break goes in front of all of it — a
        // djot `{…}` line above it included, or the typed text would take
        // the block's attributes.
        let at = match side {
            MediaStop::Before => span.start,
            MediaStop::After => at,
        };
        if !self.splice(at, at, "\n\n", EditKind::Other) {
            return;
        }
        // The break is part of the keystroke, not an edit of its own: leave the
        // run marked as typing so the character about to arrive folds into it and
        // one undo puts the document back the way it was found. (A paste, or a
        // multi-character insert, is `EditKind::Other` and stays its own step —
        // as it would have been anywhere else in the document.)
        self.last_edit_kind = Some(EditKind::Insert);
        if side == MediaStop::Before {
            // The break went in above the picture and the caret rode to the end
            // of it — which is still hard against the picture. Step back onto the
            // blank line it opened, so the text lands above rather than in front.
            self.caret = at;
        }
    }

    /// Which of a block atom's two caret stops `at` is — a block picture's
    /// ([`VisualMap::block_media_stop`]) or a block leaf directive's
    /// ([`VisualMap::block_directive_stop`]) — with the span that is the atom
    /// whole. In djot that span reaches back over a `{…}` attribute line
    /// written above the block: the line is the block's markup, drawn as
    /// nothing, and a paragraph opened between it and the block would take
    /// the attributes, as a delete that left it would hand them to the block
    /// below.
    ///
    /// Reads the map, so the caller rebuilds it first.
    fn block_atom_stop(&mut self, at: usize) -> Option<(MediaStop, Range<usize>)> {
        let (side, mut span) = self
            .vmap
            .block_media_stop(at)
            .or_else(|| self.vmap.block_directive_stop(at))?;
        if self.format == Format::Djot {
            let starts_here: Vec<NodeId> = self
                .nodes()
                .into_iter()
                .filter(|n| n.span.start == span.start)
                .map(|n| n.id)
                .collect();
            let attrs = self.editor.document().ok().and_then(|mut d| {
                starts_here
                    .into_iter()
                    .find_map(|id| d.attrs_span(id).ok().flatten())
            });
            if let Some(a) = attrs.filter(|a| a.end <= span.start) {
                span.start = a.start;
            }
        }
        Some((side, span))
    }

    /// A delete key pressed at one of a block picture's or a block leaf
    /// directive's two caret stops, handled as the block being an *atom* rather
    /// than a run of bytes. Returns whether the key was consumed.
    ///
    /// The caret rests in front of a block image and just past it, never inside
    /// its markup — which the rendered view doesn't show. So the byte a delete
    /// key nominally takes there is one the writer cannot see, and taking it
    /// leaves the picture as broken markup rather than as anything anyone asked
    /// for: Backspace at the stop past `![](p.png)` removes the closing paren, and
    /// a photo becomes the literal text `![](p.png`. That is how a picture goes
    /// missing from a document with nobody having touched it — the same
    /// dissolution [`open_paragraph_at_block_edge`](Self::open_paragraph_at_block_edge)
    /// prevents from the typing side, and it cost this repository's own test vault
    /// a photo before it was found.
    ///
    /// So the key aimed *at* the picture deletes the picture, whole — Backspace
    /// when it is behind the caret, Delete when it is in front — which is what
    /// every editor does with an embed, and one undo away. The key aimed *away*
    /// from it would otherwise delete the paragraph break and merge a neighbour
    /// into the picture's own paragraph, which dissolves it just as surely; it
    /// steps the caret over the boundary instead and leaves the
    /// next press to delete in the block it has reached — the same "first press
    /// steps out of the atom, second press deletes" every delete key here gets,
    /// word-deletes included (⌥⌫ in front of a picture is aimed at the prose
    /// above, and reaches it on the second press rather than taking the break and
    /// the picture with it on the first).
    pub(super) fn delete_around_block_atom(&mut self, forward: bool) -> bool {
        // The map answers about offsets, so it has to be this revision's — see
        // the same call in `open_paragraph_at_block_edge`.
        self.rebuild_map();
        let Some((side, span)) = self.block_atom_stop(self.caret) else {
            return false;
        };
        let aimed_at_it = side
            == if forward {
                MediaStop::Before
            } else {
                MediaStop::After
            };
        if !aimed_at_it {
            let over = if forward {
                self.vmap.stop_after(self.caret)
            } else {
                self.vmap.stop_before(self.caret)
            };
            if let Some(off) = over.filter(|&o| o >= self.caret_floor()) {
                self.caret = off;
                self.anchor = None;
                self.goal_col = None;
            }
            return true;
        }
        // Take the break that held the picture apart from its neighbour with it,
        // so the delete doesn't leave a blank paragraph standing where the
        // picture was. The last arm is a picture that is the whole document.
        let (from, to) = if self.source[..span.start].ends_with("\n\n") {
            (span.start - 2, span.end)
        } else if self.source[span.end..].starts_with("\n\n") {
            (span.start, span.end + 2)
        } else {
            (span.start, span.end)
        };
        self.splice(from.max(self.caret_floor()), to, "", EditKind::Other);
        true
    }

    /// The Hidden-mode typing path: replace any selection, then insert `text`
    /// escaped so it stays literal. When it replaces a selection the two edits
    /// fold into one undo step, so an overwrite undoes atomically (and restores
    /// the selection) exactly as a plain one does.
    fn insert_literal_typed(&mut self, text: &str) {
        let kind = typed_edit_kind(text);
        match self.selection() {
            Some((s, e)) => {
                if !self.splice_selection(s, e, "", EditKind::Other) {
                    return;
                }
                // Typing over a whole marked run takes its delimiters with it
                // (the empty content couldn't hold them — see
                // `repair_mark_edges`) and leaves its marks armed at the caret.
                // The text taking the run's place inherits them, exactly as it
                // would have by landing inside a run that survived.
                let pending = self.pending_here();
                if !pending.is_empty() && !text.trim().is_empty() {
                    self.insert_with_marks(self.caret, text, pending);
                    return;
                }
                self.insert_literal_at(self.caret, text, kind, true);
            }
            None => {
                self.insert_literal_at(self.caret, text, kind, false);
            }
        }
    }

    /// The sticky-mark delta that is live right now: the marks armed by [`toggle`]
    /// at a collapsed caret, but only while the caret still stands where they
    /// were armed and nothing is selected. Empty otherwise, so a stale delta
    /// never styles text it wasn't meant for.
    pub(super) fn pending_here(&self) -> InlineMarks {
        if self.anchor.is_none() && self.pending_at == Some(self.caret) {
            self.pending_marks
        } else {
            InlineMarks::empty()
        }
    }

    /// Drop the armed sticky marks — any caret motion, selection, or edit does
    /// this, so "start bold here" only ever applies at the exact spot it was
    /// asked for.
    pub(super) fn clear_pending(&mut self) {
        self.pending_marks = InlineMarks::empty();
        self.pending_at = None;
    }

    /// Insert `text` at `at` carrying the armed sticky `marks`: a mark not yet in
    /// force is wrapped around the freshly typed text; a mark the caret already
    /// stands inside is *shed* — the text is inserted past the run's end so it
    /// lands unmarked ("type normally again"). The caret comes to rest inside any
    /// added runs, so continued typing inherits the marks with no re-wrapping,
    /// and the delta is cleared: the marks now live in the document, not here.
    fn insert_with_marks(&mut self, at: usize, text: &str, marks: InlineMarks) {
        let base = self.mark_spans_at(at);
        let base_set: InlineMarks = base.iter().map(|(k, _)| *k).collect();
        // Nothing to shed, and a run of exactly these marks standing just behind
        // the caret: carry on writing *that* run rather than opening a second
        // one beside it.
        if base_set.is_empty() && self.rejoin_run(at, text, marks) {
            return;
        }
        // Shed the marks we're turning off: step the insertion point past the
        // end of each run the caret sits in, so the new text falls outside it.
        let mut ins_at = at;
        for (kind, span) in &base {
            if marks.contains(*kind) {
                ins_at = ins_at.max(span.end);
            }
        }
        if !self.splice_exact(ins_at, ins_at, text, EditKind::Other) {
            return;
        }
        // The plain splice inserted exactly `text` at `ins_at`; that byte range
        // is the content every added mark wraps.
        let (mut cs, mut ce) = (ins_at, ins_at + text.len());
        for kind in marks.iter() {
            if !base_set.contains(kind) {
                let (ncs, nce) = self.wrap_span(cs, ce, kind);
                cs = ncs;
                ce = nce;
            }
        }
        self.caret = ce.min(self.source.len());
        self.anchor = None;
        self.last_edit_kind = None;
        // Realised: the marks are in the document now, and the caret sits inside
        // them, so there is no delta left to carry. Arm nothing, but remember the
        // spot so a *further* toggle before typing starts a clean delta here.
        self.pending_marks = InlineMarks::empty();
        self.pending_at = Some(self.caret);
        self.clamp_caret();
        self.record_caret();
    }

    /// Carry on the marked run just behind `at` — moving its closing delimiters
    /// out past the new text — instead of opening a second run of the same marks
    /// beside it. Returns whether it did.
    ///
    /// This is the far half of the mark-edge rule (see [`splice`](Self::splice)).
    /// A space typed after a bold word steps the caret out of the run, because
    /// `**bold **` is not bold; the next character has to step back *in*, or the
    /// writer who typed one bold phrase is left with `**bold** **and**` — two
    /// runs that read the same to a reader but spell the file in a way nobody
    /// wrote. Only whitespace may stand in the gap (a run doesn't reach across
    /// words it isn't marking), and the marks behind it must be exactly the ones
    /// armed — a run of *some* other kind is a neighbour, not this phrase.
    fn rejoin_run(&mut self, at: usize, text: &str, marks: InlineMarks) -> bool {
        if text.is_empty() || text.trim() != text {
            return false;
        }
        let gap_at = self.source[..at].trim_end_matches([' ', '\t']).len();
        // Walk in through the delimiters stacked at that point, innermost last:
        // `***both*** ` closes two runs with one `***`, and rejoining means
        // getting behind all of them.
        let (mut cut, mut kinds) = (gap_at, InlineMarks::empty());
        while let Some((kind, content_end)) = self
            .editor
            .ancestors_at(prev_boundary(&self.source, cut))
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m.span.end == cut)
            .find_map(|m| Some((inline_kind(&m.kind)?, m.content_span.clone()?.end)))
        {
            if content_end >= cut {
                break; // a mark with no closing delimiter to step behind
            }
            kinds.insert(kind);
            cut = content_end;
        }
        if cut == gap_at || kinds != marks {
            return false;
        }
        // Re-spell the tail: the gap, then the new text, then the delimiters that
        // used to close in front of them — read out of the document rather than
        // written from a table, so whatever twig spells them with is what moves.
        let tail = format!(
            "{}{text}{}",
            &self.source[gap_at..at],
            &self.source[cut..gap_at]
        );
        if !self.splice_exact(cut, at, &tail, EditKind::Other) {
            return false;
        }
        self.caret = (cut + (at - gap_at) + text.len()).min(self.source.len());
        self.anchor = None;
        self.last_edit_kind = None;
        self.pending_marks = InlineMarks::empty();
        self.pending_at = Some(self.caret);
        self.clamp_caret();
        self.record_caret();
        true
    }

    /// Insert typed whitespace at a caret with sticky marks armed. Whitespace is
    /// never itself wrapped: a mark around a space draws nothing a reader can
    /// see, and in Markdown and Djot it draws its own delimiters instead
    /// (`** **`). So the space goes in unmarked — outside any run the armed
    /// marks are shedding — and the marks stay armed for the character after it,
    /// which rejoins the run (see [`rejoin_run`](Self::rejoin_run)).
    fn insert_space_with_marks(&mut self, at: usize, text: &str, marks: InlineMarks) {
        let base = self.mark_spans_at(at);
        // What the *next* character carries: the armed delta resolved against the
        // marks in force here, which the space must not quietly drop.
        let want = base
            .iter()
            .map(|(k, _)| *k)
            .collect::<InlineMarks>()
            .xor(marks);
        let mut ins_at = at;
        for (kind, span) in &base {
            if marks.contains(*kind) {
                ins_at = ins_at.max(span.end);
            }
        }
        if !self.splice(ins_at, ins_at, text, typed_edit_kind(text)) {
            return;
        }
        self.rearm(want);
        self.record_caret();
    }

    /// Wrap `[s, e)` in `kind` via twig and return the byte span the *content*
    /// (not the delimiters) occupies afterwards. Markdown/Djot inline delimiters
    /// are symmetric (`**`…`**`, `_`…`_`, `` ` ``…`` ` ``), so the bytes twig
    /// added split evenly around the content — half the growth on each side.
    fn wrap_span(&mut self, s: usize, e: usize, kind: InlineKind) -> (usize, usize) {
        // The read-only gate — this door reaches twig without the splice.
        if self.read_only {
            return (s, e);
        }
        match self.editor.toggle_inline(s, e, kind) {
            Ok(change) => {
                self.last_edit_kind = None;
                self.refresh();
                self.dirty = self.source != self.clean_source;
                let added = (change.new.end - change.new.start).saturating_sub(e - s);
                let half = added / 2;
                (change.new.start + half, change.new.end - half)
            }
            // Unsupported here (e.g. mark on Markdown): leave the text unwrapped
            // rather than lose the keystroke.
            Err(e2) => {
                self.status = Some(format!("{kind:?}: {e2}"));
                (s, e)
            }
        }
    }

    /// The safe offset to splice a block-level break at, given a caret that may
    /// sit exactly between an inline mark's content and its own closing
    /// delimiter (`content_span.end == off < span.end` for some enclosing mark
    /// — the WYSIWYG caret's natural resting place at the end of `**bold**`
    /// with nothing following it on the line: the closing `**` renders no
    /// glyph of its own, so the caret's "end of line" offset lands right
    /// before it). Splicing a paragraph/list/quote break at `off` itself would
    /// sever the delimiter from its content, stranding it alone on the new
    /// line. Walks out to the *outermost* such mark's `span.end` instead, so
    /// nested marks closing at the same point (`**_x_**`) all clear together.
    /// A no-op everywhere else — mid-run, or past real trailing content, no
    /// mark's `content_span` ends exactly at `off`.
    pub(super) fn skip_trailing_close_delims(&mut self, off: usize) -> usize {
        let off = off.min(self.source.len());
        let runs = self.run_span_ids();
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| hides_delims(m, &runs))
            .filter(|m| off < m.span.end && m.content_span.as_ref().is_some_and(|c| c.end == off))
            .map(|m| m.span.end)
            .max()
            .unwrap_or(off)
    }

    /// The offset a *delete* aimed at the character before `off` should stop at,
    /// when `off` is the start of a run's text and the bytes behind it are that
    /// run's opening delimiter. The rich view draws no glyph for a `**`, so the
    /// byte behind the caret at the start of a bold word is not a character the
    /// writer can see, let alone one they aimed Backspace at: taking it leaves
    /// `a *bold** c` — the styling gone and a literal asterisk in its place. The
    /// delete steps over the whole delimiter to the visible character in front of
    /// it instead. Walks out to the *outermost* mark opening there, so
    /// `**_x_**` clears every delimiter at once, and is a no-op anywhere else.
    pub(super) fn skip_leading_open_delims(&mut self, off: usize) -> usize {
        let off = off.min(self.source.len());
        let runs = self.run_span_ids();
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| hides_delims(m, &runs))
            .filter(|m| {
                m.span.start < off && m.content_span.as_ref().is_some_and(|c| c.start == off)
            })
            .map(|m| m.span.start)
            .min()
            .unwrap_or(off)
    }

    /// `off` moved *inside* the run whose closing delimiters end there — the
    /// other offset the rich view draws in the same place, since a `**` renders
    /// no glyph of its own. `**bold**` has a caret home on each side of its
    /// closing delimiter, one column apart on screen and eight bytes and a whole
    /// run apart in the file, and a plain ← lands on the outer one whenever a
    /// space follows the phrase. The inner one is what the writer is pointing at
    /// there: the end of their bold word. Walks in through every mark closing at
    /// that point, innermost last, so `***both***` lands inside both. A no-op
    /// anywhere else — mid-run, or in prose, no mark's span ends at `off`.
    pub(super) fn step_inside_close_delims(&mut self, off: usize) -> usize {
        let mut off = off.min(self.source.len());
        let runs = self.run_span_ids();
        loop {
            let inner = self
                .editor
                .ancestors_at(prev_boundary(&self.source, off))
                .unwrap_or_default()
                .into_iter()
                .filter(|m| hides_delims(m, &runs) && m.span.end == off)
                .filter_map(|m| m.content_span.clone().map(|c| c.end))
                .filter(|&end| end < off)
                .max();
            match inner {
                Some(end) => off = end,
                None => return off,
            }
        }
    }

    /// The mirror at the opening edge: `off` moved inside the run whose
    /// delimiters *start* there, onto the first character of its text. See
    /// [`step_inside_close_delims`](Self::step_inside_close_delims).
    pub(super) fn step_inside_open_delims(&mut self, off: usize) -> usize {
        let mut off = off.min(self.source.len());
        let runs = self.run_span_ids();
        loop {
            let inner = self
                .editor
                .ancestors_at(off)
                .unwrap_or_default()
                .into_iter()
                .filter(|m| hides_delims(m, &runs) && m.span.start == off)
                .filter_map(|m| m.content_span.clone().map(|c| c.start))
                .filter(|&start| start > off)
                .min();
            match inner {
                Some(start) => off = start,
                None => return off,
            }
        }
    }

    /// The ids of the document's attributed run spans — the inline
    /// `Container`s [`wysiwyg::is_run_span`] picks out — for [`hides_delims`],
    /// which sees an ancestor chain and so only a kind. Read once per gesture,
    /// not once per step of a walk.
    fn run_span_ids(&mut self) -> Vec<NodeId> {
        self.nodes()
            .iter()
            .filter(|n| wysiwyg::is_run_span(n))
            .map(|n| n.id)
            .collect()
    }

    /// The attributed span whose text is exactly `content` — the whole of
    /// `<span …>i</span>`'s `i`, or nothing at all when `content` is empty
    /// and sits between the tags of `<span …></span>` — as the whole range
    /// spelling the span: the node's span, widened to its attribute block
    /// where the format writes that outside the node, as djot's
    /// `[i]{data-size="large"}` does. `None` for any other range, including
    /// part of a span's text.
    ///
    /// An empty span has an interior of no bytes, or no known interior at
    /// all: twig gives Markdown's `<span …></span>` the first and djot's
    /// `[]{…}` the second, and the chain already says the offset is inside.
    pub(super) fn run_span_of_content(&mut self, content: Range<usize>) -> Option<Range<usize>> {
        let runs = self.run_span_ids();
        let m = self
            .editor
            .ancestors_at(content.start)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| runs.contains(&NodeId(m.node_id)))
            .find(|m| match &m.content_span {
                Some(c) => *c == content,
                None => content.is_empty(),
            })?;
        let mut range = m.span;
        if let Some(attrs) = self
            .editor
            .document()
            .ok()
            .and_then(|mut d| d.attrs_span(NodeId(m.node_id)).ok().flatten())
        {
            range.start = range.start.min(attrs.start);
            range.end = range.end.max(attrs.end);
        }
        Some(range)
    }

    /// The attributed block whose whole text is exactly `content` — the `T`
    /// of Markdown's `<div class="center">\n\nT\n\n</div>` or djot's
    /// `{.center}\nT` — as the range a delete that takes that text takes with
    /// it: the whole `<div>` when the block is all the div holds, or the
    /// `{…}` line down to the end of the text. The block version of
    /// [`run_span_of_content`](Self::run_span_of_content), for the same
    /// reason: a paragraph with no text is no block, so the div would stand
    /// around nothing and the `{…}` line above nothing, and a from-scratch
    /// map gives neither a caret home — the `T`'s row is gone with the `T`.
    /// `None` for a block with more text, a div holding more, a heading (an
    /// empty `# ` is still a heading), and a format whose attributes are the
    /// block's own tag (HTML's `<p class="center"></p>` is still a
    /// paragraph).
    pub(super) fn attributed_block_of_content(
        &mut self,
        content: Range<usize>,
    ) -> Option<Range<usize>> {
        if content.is_empty() || !matches!(self.format, Format::Markdown | Format::Djot) {
            return None;
        }
        let nodes = self.nodes();
        let block = nodes
            .iter()
            .filter(|n| n.kind == Kind::Para)
            .find(|n| n.content_span.as_ref() == Some(&content))?;
        match self.format {
            Format::Djot => {
                let attrs = self
                    .editor
                    .document()
                    .ok()
                    .and_then(|mut d| d.attrs_span(block.id).ok().flatten())?;
                (attrs.end <= block.span.start).then_some(attrs.start..content.end)
            }
            _ => {
                let div = block
                    .parent
                    .and_then(|p| nodes.iter().find(|n| n.id == p))
                    .filter(|p| wysiwyg::element_tag(p) == Some("div"))?;
                let alone = nodes.iter().filter(|n| n.parent == Some(div.id)).count() == 1;
                alone.then(|| div.span.clone())
            }
        }
    }

    /// The inline mark kinds whose span covers `off`, each with that span — the
    /// span-carrying sibling of [`marks_at`](Self::marks_at), which reports node
    /// ids instead. Used to shed a mark by stepping past the end of its run.
    fn mark_spans_at(&mut self, off: usize) -> Vec<(InlineKind, std::ops::Range<usize>)> {
        let off = off.min(self.source.len());
        self.editor
            .ancestors_at(off)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| off < m.span.end)
            .filter_map(|m| inline_kind(&m.kind).map(|k| (k, m.span.clone())))
            .collect()
    }

    /// Insert clipboard `text` at the caret, replacing the selection if there is
    /// one — always its own undo step, whatever its length.
    ///
    /// Provenance is the whole point, and only the caller has it. `insert` reads
    /// a lone character as a keystroke and folds it into the run around it,
    /// which is right for typing and wrong for a one-character paste: that paste
    /// would vanish mid-run on an undo it was never part of, and the characters
    /// the user actually typed would go with it. Length can't tell the two
    /// apart — `⌘V` of `x` and typing `x` are the same string — so the door the
    /// caller comes through is what says which happened.
    pub fn paste(&mut self, text: &str) {
        // Pasting against a block picture or a table's end joins the block
        // exactly as typing does, and for the same reason — see
        // `open_paragraph_at_block_edge`.
        self.open_paragraph_at_block_edge(text);
        let (s, e) = self.selection().unwrap_or((self.caret, self.caret));
        self.splice_selection(s, e, text, EditKind::Other);
    }

    /// Replace `[start, end)` with `text` as one step of an IME composition —
    /// the same splice as [`edit`](Self::edit), but marked so the run of steps
    /// folds into a single undo.
    ///
    /// A composition is *one* act of writing. Typing `かんじ` and picking 感じ is a
    /// dozen calls here, each replacing the last one's provisional bytes, and an
    /// undo step per call means undoing a word means pressing ⌘Z until the reading
    /// unspools backwards through kana — the intermediate states were never text
    /// the user wrote. Only the frontend knows a call is provisional (the bytes
    /// look like any other edit), so the door the caller comes through is what
    /// says so, exactly as it is for [`paste`](Self::paste) versus
    /// [`insert`](Self::insert).
    ///
    /// Pair with [`end_composition`](Self::end_composition), or the *next*
    /// composition folds into this one.
    pub fn edit_composing(&mut self, start: usize, end: usize, text: &str) {
        self.splice(start, end, text, EditKind::Compose);
    }

    /// Close the open composition run, so the next one is its own undo step.
    /// Call when the IME commits or withdraws a composition.
    ///
    /// Only clears a *composition* run: a frontend that reports an end it never
    /// began (some IMEs unmark unprompted) would otherwise split the run of
    /// typing around it into two undo steps for no reason the user can see.
    pub fn end_composition(&mut self) {
        if self.last_edit_kind == Some(EditKind::Compose) {
            self.last_edit_kind = None;
        }
    }
}
