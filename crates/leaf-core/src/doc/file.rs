//! The file on disk: save, save as, reload, and noticing an external change.

use super::*;

impl Doc {
    // ── the file ──────────────────────────────────────────────────────────────

    #[cfg(feature = "fs")]
    pub fn save(&mut self) {
        if self.is_untitled() {
            // No path to write and no name to invent: ⌘S on an untitled document
            // is a Save As, and only a frontend has a picker to ask with. Say so
            // rather than failing at the filesystem with an empty path.
            self.status = Some("untitled — save as…".into());
            return;
        }
        let path = self.path.clone();
        if self.write(&path) {
            self.mark_saved();
        }
    }

    /// Save As: write the document to `path` and *move* it there — `self.path`
    /// becomes `path`, and every later [`Doc::save`] writes the new file. That's
    /// what Save As means; a copy would leave the user editing a document whose
    /// name is no longer where their keystrokes go.
    ///
    /// The move only happens if the bytes actually landed. A failed write leaves
    /// the path, `dirty`, and the disk watermark exactly as they were, with the
    /// same `save failed: …` status a failed [`Doc::save`] sets — the document
    /// must never come away believing it was saved.
    ///
    /// An existing `path` is overwritten, and the caller is the one that knows
    /// whether to ask first: a Save As picker has already run that prompt, and a
    /// second confirmation from down here would be the same question twice.
    ///
    /// `format` does **not** follow the new extension. The buffer is parsed as
    /// the format it was opened with, and re-reading it as another one is a
    /// conversion — a different, lossy operation that would throw away the undo
    /// history — not a rename. So `notes.md` saved as `notes.dj` holds Markdown
    /// in a `.dj` file, and `format_name()` keeps honestly saying `markdown`
    /// until it's reopened.
    #[cfg(feature = "fs")]
    pub fn save_as(&mut self, path: PathBuf) {
        if !self.write(&path) {
            return;
        }
        self.path = path;
        self.mark_saved();
    }

    /// Put `source` on disk at `path`, reporting whether it got there. The one
    /// place leaf writes a document, so a save and a Save As can't disagree
    /// about what a failure looks like.
    #[cfg(feature = "fs")]
    fn write(&mut self, path: &Path) -> bool {
        match std::fs::write(path, self.source.as_bytes()) {
            Ok(()) => true,
            Err(e) => {
                self.status = Some(format!("save failed: {e}"));
                false
            }
        }
    }

    /// Re-base the document's saved watermark to the current bytes: clears
    /// `dirty`, records `source` as the new clean state (so undoing back to here
    /// clears the flag again), and re-stamps the on-disk hash.
    ///
    /// [`Doc::save`]/[`Doc::save_as`] call this after a write lands. It is also
    /// the hook a **filesystem-free host** calls itself once it has persisted
    /// [`Doc::source`] its own way (a browser download, `localStorage`, a backend
    /// `PUT`) — which is why it is public and touches no filesystem: the bytes
    /// are already where that host wants them, and this just tells the model they
    /// are safe.
    pub fn mark_saved(&mut self) {
        self.clean_source = self.source.clone();
        self.dirty = false;
        // The bytes on disk are now ours, so this is the new watermark: without
        // re-stamping it, every save would report its own work as an external
        // change forever after.
        self.disk_hash = Some(hash_bytes(self.source.as_bytes()));
        self.status = Some(format!("saved {}", self.file_name()));
    }

    /// [`Doc::mark_saved`] for a host whose write is not instantaneous: `saved`
    /// is the source it read before writing, and the edits made since are
    /// still unsaved.
    ///
    /// A host that reads [`Doc::source`], hands it to a write that takes a
    /// while, and then calls `mark_saved` loses whatever was typed meanwhile:
    /// the flag is cleared over text that never reached the disk, so nothing
    /// saves it again. Here `saved` becomes the clean state and the disk
    /// watermark, and `dirty` is whether the buffer has moved on from it.
    pub fn mark_saved_as(&mut self, saved: &str) {
        self.clean_source = saved.to_string();
        self.dirty = self.source != self.clean_source;
        self.disk_hash = Some(hash_bytes(saved.as_bytes()));
        if !self.dirty {
            self.status = Some(format!("saved {}", self.file_name()));
        }
    }

    /// What the file looks like now against the bytes leaf last read or wrote.
    ///
    /// Reads the file and hashes it (see `disk_hash` for why it isn't an mtime),
    /// so this is a filesystem round-trip, not a per-frame question — ask it
    /// when a window regains focus, on a timer, or before a save.
    ///
    /// This *only* reports the file. Whether the document also has unsaved edits
    /// is `dirty`, and the interesting case is the conjunction: `dirty` plus
    /// [`DiskState::Changed`] means a save overwrites someone's work and a
    /// [`Doc::reload`] discards the user's. leaf-core deliberately won't choose —
    /// it has no way to ask — so it hands a frontend both halves and lets it put
    /// the question to the person who can answer it.
    #[cfg(feature = "fs")]
    pub fn disk_state(&self) -> DiskState {
        let Some(want) = self.disk_hash else {
            return DiskState::Untitled;
        };
        match std::fs::read(&self.path) {
            Ok(bytes) if hash_bytes(&bytes) == want => DiskState::Unchanged,
            Ok(_) => DiskState::Changed,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DiskState::Missing,
            Err(_) => DiskState::Unreadable,
        }
    }

    /// Re-read the file and replace the document with what's there — the other
    /// answer to a [`DiskState::Changed`].
    ///
    /// **Discards unsaved changes, unconditionally.** It doesn't check `dirty`
    /// first: a frontend that wants to protect unsaved work asks (`dirty` +
    /// [`Doc::disk_state`]) *before* calling this, and one reloading a clean
    /// document shouldn't have to argue with a guard.
    ///
    /// **The undo history survives, and the reload is one step in it.** The
    /// whole buffer is spliced with the file's bytes through the same door every
    /// other edit goes through, as an [`EditKind::Other`] that coalesces with
    /// nothing on either side — so ^Z after a formatter or a `git checkout` has
    /// swapped the document out from under a reader gives them back what they
    /// were looking at, marked dirty, and ^Z again carries on into whatever they
    /// had done before it. This used to build a fresh parse and drop the stack,
    /// on the reasoning that twig's history belongs to the buffer and these are
    /// different bytes; that is true of *rebasing* a step onto them and not of
    /// recording the swap itself as one, which is all this is. A splice twig
    /// won't take falls back to the fresh parse, and only that path still costs
    /// the history.
    ///
    /// The caret keeps its byte offset, clamped to the new length; the selection
    /// is dropped. Anything cleverer would be a lie: leaf doesn't know how the
    /// file changed, so it can't know where the caret "still" is. Clamping keeps
    /// it where the user left it in the common case (a change further down the
    /// file, or none in the text they're sitting in), and never puts it
    /// somewhere invalid. A selection has two such offsets and no such excuse —
    /// silently reinterpreting one over changed bytes would arm the *next*
    /// keystroke to delete something the user never selected.
    ///
    /// Nothing is touched unless the whole reload succeeds; a failure leaves the
    /// document alone with a status.
    #[cfg(feature = "fs")]
    pub fn reload(&mut self) {
        if self.is_untitled() {
            self.status = Some("no file to reload".into());
            return;
        }
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) => {
                self.status = Some(format!("reload failed: {e}"));
                return;
            }
        };
        let Ok(source) = String::from_utf8(bytes) else {
            self.status = Some("reload failed: file is not UTF-8".into());
            return;
        };
        // Already these bytes — someone saved a file back unchanged, or leaf's
        // own write is being read back. Re-baseline against it and stop: a
        // splice of the text onto itself would put an undo step on the stack for
        // something nobody did.
        if source == self.source {
            self.disk_hash = Some(hash_bytes(source.as_bytes()));
            self.clean_source = source;
            self.dirty = false;
            self.status = Some(format!("reloaded {}", self.file_name()));
            return;
        }
        let caret = self.caret;
        // The pre-reload caret, so undoing the swap puts it back where the
        // reader was standing — the same bracketing `splice_exact` does.
        self.record_caret();
        // Only what differs, so the ops a host takes are the change and not
        // the whole file, and the undo step holds no more.
        if self.editor.apply(&Op::diff(&self.source, &source)).is_ok() {
            self.refresh();
        } else {
            // twig wouldn't take the splice. Start over from the bytes, which is
            // what this always did, and is the one path that still costs the
            // history — `format` is the format this document *is*, not what the
            // (unchanged) name now says, see `save_as`.
            match new_editor(source.as_bytes(), self.format) {
                Ok(editor) => {
                    self.editor = editor;
                    // A fresh editor, and so a fresh history — with no
                    // step in it for an open group to fold into.
                    self.undo_steps = 0;
                    self.redo_steps = 0;
                    if let Some(g) = &mut self.undo_group {
                        g.has_step = false;
                    }
                    let before = std::mem::replace(&mut self.source, source.clone());
                    // A new editor, whose revisions start again: the change
                    // is the diff of the two texts.
                    self.record_ops(&before);
                    // Not going through `refresh`, so the revision has to move
                    // here or every frontend keeps painting the old file from
                    // cache.
                    self.revision += 1;
                }
                Err(e) => {
                    self.status = Some(format!("reload failed: {e}"));
                    return;
                }
            }
        }
        self.disk_hash = Some(hash_bytes(source.as_bytes()));
        self.clean_source = self.source.clone();
        self.caret = caret.min(self.source.len());
        self.anchor = None;
        self.goal_col = None;
        self.last_edit_kind = None;
        self.dirty = false;
        self.status = Some(format!("reloaded {}", self.file_name()));
        self.clamp_caret();
        // And the post-reload caret, so a redo restores it.
        self.record_caret();
    }

    /// Re-read the source from twig after it has changed the document. The one
    /// funnel every edit, undo, and redo comes through — so it's where the
    /// revision moves, and anything cached against the text dies here.
    pub(super) fn refresh(&mut self) {
        let before = match self.editor.source_str() {
            Ok(s) => std::mem::replace(&mut self.source, s),
            Err(_) => self.source.clone(),
        };
        self.record_ops(&before);
        self.revision += 1;
        // An edit is a step onto the history and the end of anything undone;
        // `undo`/`redo` come through here too and correct this after.
        self.undo_steps = (self.undo_steps + 1).min(UNDO_CAP);
        self.redo_steps = 0;
        // Inside a group, fold each step into the group's first as it lands.
        // Not for `undo`/`redo`, which close the group before they get here.
        if let Some(g) = &mut self.undo_group {
            if g.has_step {
                self.fold_last_undo();
            } else {
                g.has_step = true;
            }
        }
        self.clamp_caret();
    }

    /// Whether [`undo`](Self::undo) has a step to take back — for a native
    /// Edit menu to enable its item by, and exactly when `undo` would move.
    pub fn can_undo(&self) -> bool {
        !self.read_only && self.undo_steps > 0
    }

    /// Whether [`redo`](Self::redo) has an undone step to restore.
    pub fn can_redo(&self) -> bool {
        !self.read_only && self.redo_steps > 0
    }
}
