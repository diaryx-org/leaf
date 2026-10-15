//! Undo and redo, and how edits group into one step.

use super::*;

impl Doc {
    // ── undo / redo ───────────────────────────────────────────────────────────
    // twig owns the history of *bytes* (it owns the buffer) and now carries the
    // caret through it too: `record_caret` stashes each state's caret in twig's
    // opaque per-step blob, and undo/redo hand it back with the source they
    // restore. So leaf keeps no history of its own — no parallel stacks to march
    // in lockstep and silently drift out of it.

    /// Undo the last edit step (⌘Z / ^Z), putting the caret and selection back
    /// where they were when that step began. Whether it did: `false` when
    /// there was nothing to undo, the document is read-only, or twig refused —
    /// the answer a host composing several histories into one walks on by.
    pub fn undo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        // Stepping through the history ends a group: what comes after is not
        // part of the step just taken back.
        self.close_undo_group();
        let (undone, redoable) = (self.undo_steps, self.redo_steps);
        match self.editor.undo() {
            Ok(Some(change)) => {
                self.after_history(change);
                // `refresh` counted the restore as an edit; it was a step back.
                self.undo_steps = undone.saturating_sub(1);
                self.redo_steps = redoable + 1;
                true
            }
            Ok(None) => {
                self.undo_steps = 0;
                self.status = Some("nothing to undo".into());
                false
            }
            Err(e) => {
                self.status = Some(format!("undo: {e}"));
                false
            }
        }
    }

    /// Redo the last undone edit step (⇧⌘Z / ^Y), putting the caret and
    /// selection back where that step originally left them. Whether it did,
    /// as [`undo`](Self::undo) reports.
    pub fn redo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        self.close_undo_group();
        let (undone, redoable) = (self.undo_steps, self.redo_steps);
        match self.editor.redo() {
            Ok(Some(change)) => {
                self.after_history(change);
                // `refresh` counted the restore as an edit; it was a step forward.
                self.undo_steps = (undone + 1).min(UNDO_CAP);
                self.redo_steps = redoable.saturating_sub(1);
                true
            }
            Ok(None) => {
                self.redo_steps = 0;
                self.status = Some("nothing to redo".into());
                false
            }
            Err(e) => {
                self.status = Some(format!("redo: {e}"));
                false
            }
        }
    }

    /// Fold the edit just made into the undo step before it — twig's
    /// `coalesce_last_undo`, with the `undo_steps` mirror following
    /// it. twig merges only when there are two steps to merge. Call it after
    /// [`refresh`](Self::refresh) has counted the edit being folded.
    ///
    /// Inside an [undo group](Self::begin_undo_group) it does nothing: the
    /// group's edits are already one step, which [`refresh`](Self::refresh)
    /// folds as they come, so a fold here could only reach across the group's
    /// start into the step before it.
    pub(super) fn coalesce_last_undo(&mut self) {
        if self.undo_group.is_some() {
            return;
        }
        self.fold_last_undo();
    }

    /// twig's `coalesce_last_undo`, unconditionally, with the mirror following.
    pub(super) fn fold_last_undo(&mut self) {
        if self.editor.coalesce_last_undo().is_ok() && self.undo_steps >= 2 {
            self.undo_steps -= 1;
        }
    }

    /// Open an undo group: every edit from here to the matching
    /// [`end_undo_group`](Self::end_undo_group) is one step, which a single
    /// [`undo`](Self::undo) takes back whole and a single [`redo`](Self::redo)
    /// puts back — a Replace All over twelve matches, or a Writing Tools
    /// session, rather than twelve presses of ⌘Z.
    ///
    /// Groups nest, and only the outermost end closes one. The step is folded
    /// as each edit lands (twig's `coalesce_last_undo`, one edit at a time),
    /// so a group of a thousand edits holds one step on the history and never
    /// meets twig's cap. Nothing typed before the group folds into it, and
    /// nothing typed after: it is a step of its own, even when it is a single
    /// edit. A group no edit landed in leaves no step at all.
    ///
    /// An [`undo`](Self::undo) or [`redo`](Self::redo) closes any open group,
    /// whatever its depth — the history has moved, and an edit made after it
    /// is not part of the step it moved past. A later `end_undo_group` is
    /// then a no-op.
    pub fn begin_undo_group(&mut self) {
        match &mut self.undo_group {
            Some(g) => g.depth += 1,
            None => {
                self.undo_group = Some(UndoGroup {
                    depth: 1,
                    has_step: false,
                });
                // The first edit inside is not a continuation of the typing
                // before it.
                self.last_edit_kind = None;
            }
        }
    }

    /// Close the undo group [`begin_undo_group`](Self::begin_undo_group)
    /// opened. A no-op when none is open.
    pub fn end_undo_group(&mut self) {
        let Some(g) = &mut self.undo_group else {
            return;
        };
        if g.depth > 1 {
            g.depth -= 1;
        } else {
            self.close_undo_group();
        }
    }

    /// Whether an undo group is open.
    pub fn in_undo_group(&self) -> bool {
        self.undo_group.is_some()
    }

    /// Close any open group, however deeply nested.
    fn close_undo_group(&mut self) {
        if self.undo_group.take().is_some() {
            // Typing after the group is not a continuation of the last edit
            // in it.
            self.last_edit_kind = None;
        }
    }

    /// Refresh the cached source and put the caret back where the step being
    /// undone/redone had it, clearing any active run.
    ///
    /// The caret comes from twig's blob for the restored state (what
    /// `record_caret` stored). `change` is only the fallback for a state with no
    /// blob — a caret at the end of the restored text, which is where this always
    /// landed before the blobs were kept. It is the edit site, not where the user
    /// was standing, so it's a floor and not the behaviour: undoing should hand
    /// back the document *and* the place you were working, which for an edit made
    /// anywhere but under the caret are two different places.
    fn after_history(&mut self, change: Change) {
        self.refresh();
        match self
            .editor
            .caret_blob()
            .ok()
            .and_then(|b| CaretState::from_blob(&b))
        {
            Some(state) => {
                self.caret = state.caret.min(self.source.len());
                self.anchor = state.anchor.map(|a| a.min(self.source.len()));
            }
            None => {
                self.caret = change.new.end.min(self.source.len());
                self.anchor = None;
            }
        }
        self.goal_col = None;
        self.last_edit_kind = None;
        self.dirty = self.source != self.clean_source;
        self.status = None;
        self.clamp_caret();
    }
}
