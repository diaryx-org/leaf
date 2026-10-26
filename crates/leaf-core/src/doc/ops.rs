//! The edits a host sends on: every change to the text as twig's retain,
//! insert and delete ops, gathered until the host takes them.
//!
//! twig reports each edit as the ops it made — the fewest for a gesture, so
//! bold over a word is two inserts and the word kept, and the span asked for
//! a keystroke. [`refresh`](Doc::refresh), the funnel every change comes
//! through, folds them in order into one list against the text the host last
//! took. A host sends that list where it sent the whole body, and a replica
//! that holds the same text applies it with [`twig::Op::apply`].

use super::*;

impl Doc {
    /// Every change to the text since the last call, or since the document
    /// was opened, as one list of ops against the text then: typing, a
    /// gesture, an undo, a [`replace_source`](Self::replace_source), a
    /// reload. Empty when nothing changed. What a host sends on instead of
    /// the whole body; [`twig::Op::apply`] of them to the text it last had
    /// gives [`source`](Self::source).
    pub fn take_ops(&mut self) -> Vec<Op> {
        self.ops_base.clone_from(&self.source);
        std::mem::take(&mut self.ops)
    }

    /// Whether the text changed since the ops were last taken.
    pub fn has_ops(&self) -> bool {
        !self.ops.is_empty()
    }

    /// Fold the change [`refresh`](Self::refresh) just read into the ops not
    /// yet taken. `before` is the text before it.
    ///
    /// twig's ops for its last edit are the change when exactly one edit
    /// landed since the last fold. Where more did (a gesture that edited and
    /// rolled back before refreshing) or the editor is a new one (a reload
    /// twig refused to splice), the change is the diff of the two texts.
    pub(super) fn record_ops(&mut self, before: &str) {
        let revision = self.editor.revision();
        let exact = revision == self.ops_revision + 1;
        self.ops_revision = revision;
        let edit = match exact.then(|| self.editor.last_ops().ok()).flatten() {
            Some(ops) => ops,
            None => Op::diff(before, &self.source),
        };
        if edit.is_empty() {
            return;
        }
        self.ops = match Op::compose(&self.ops_base, &self.ops, &edit) {
            Ok(ops) => ops,
            // Only ops that split a character twig inserted, which twig's own
            // never do; the text the host last took is still a sound start.
            Err(_) => Op::diff(&self.ops_base, &self.source),
        };
    }

    /// Bring the source to `text` by `ops`, with the caret and selection
    /// carried through every place they change — see
    /// [`replace_source`](Self::replace_source). One undo step, bracketed by
    /// the caret before and after as [`splice_exact`](Self::splice_exact)
    /// brackets one.
    pub(super) fn apply_ops(&mut self, ops: &[Op]) -> bool {
        if self.read_only {
            return false;
        }
        let (caret, anchor) = (self.caret, self.anchor);
        let (pending_marks, pending_at) = (self.pending_marks, self.pending_at);
        self.last_edit_kind = None;
        self.record_caret();
        if let Err(e) = self.editor.apply(ops) {
            self.status = Some(format!("edit: {e}"));
            return false;
        }
        self.refresh();
        self.dirty = self.source != self.clean_source;
        self.status = None;
        let splices = splices(ops);
        self.caret = carry(caret, &splices);
        self.anchor = anchor
            .map(|a| carry(a, &splices))
            .filter(|&a| a != self.caret);
        if pending_at == Some(caret) {
            self.pending_marks = pending_marks;
            self.pending_at = Some(self.caret);
        } else {
            self.clear_pending();
        }
        self.goal_col = None;
        self.clamp_caret();
        // The caret the step leaves, so a redo puts it back here too.
        self.record_caret();
        true
    }
}

/// The places `ops` change, in order, as `(start, end, inserted)`: the bytes
/// `[start, end)` of the text they read gave way to `inserted` bytes.
pub(super) fn splices(ops: &[Op]) -> Vec<(usize, usize, usize)> {
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    let mut at = 0;
    let mut open = false;
    for op in ops {
        match op {
            Op::Retain(n) => {
                at += n;
                open = false;
                continue;
            }
            Op::Insert(t) if t.is_empty() => continue,
            Op::Delete(0) => continue,
            _ => {}
        }
        if !open {
            out.push((at, at, 0));
            open = true;
        }
        let last = out.last_mut().expect("opened above");
        match op {
            Op::Insert(t) => last.2 += t.len(),
            Op::Delete(n) => {
                at += n;
                last.1 = at;
            }
            Op::Retain(_) => unreachable!(),
        }
    }
    out
}

/// Where offset `p` of the text before `splices` lands after them: past each
/// one that ends at or before it, moved by it; inside one, at the end of what
/// replaced it; otherwise where it was. One splice moves a place exactly as
/// [`Doc::substitute`] moves the caret.
pub(super) fn carry(p: usize, splices: &[(usize, usize, usize)]) -> usize {
    let mut delta: isize = 0;
    for &(start, end, inserted) in splices {
        if p >= end {
            delta += inserted as isize - (end - start) as isize;
        } else if p > start {
            return (start as isize + delta) as usize + inserted;
        } else {
            break;
        }
    }
    (p as isize + delta).max(0) as usize
}
