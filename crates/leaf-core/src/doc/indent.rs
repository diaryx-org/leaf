//! Indenting and outdenting the caret's list item or lines.

use super::*;

impl Doc {
    // ── indentation ──────────────────────────────────────────────────────────

    /// One indent level.
    ///
    /// Two spaces, not the four both frontends type for Tab today, because in a
    /// markdown document four columns isn't a width — it's a *meaning*. Four
    /// spaces at the head of a line is markdown's indented-code-block marker, so
    /// one Tab on a paragraph would reparse it into code and style it as such;
    /// two cannot, and the line stays the prose it was. Two is also exactly
    /// where a `- ` bullet's content starts, so an indented line lands under its
    /// parent item's text instead of beside it — the column a list-aware indent
    /// has to hit anyway, which keeps this width from being relitigated later.
    const INDENT: &'static str = "  ";

    /// Indent the selected lines — or the caret's line, with no selection — by
    /// one level (Tab).
    pub fn indent(&mut self) {
        // A verse line's indent is em spaces, the one leading space twig reads
        // back as an indent there — two ASCII spaces would be stripped.
        if self.verse_reindent(true) {
            return;
        }
        self.reindent(true);
        // Nesting changes an ordered list's numbering (the nested item restarts,
        // its old siblings resume) — keep the source markers in step.
        self.renumber_here();
        // Nesting an empty `-` item under a text line reparses that text as a
        // setext heading; swap the dash for a `*` before it can (a no-op unless
        // the collapse actually happened).
        self.avoid_setext_collapse();
    }

    /// Take one indent level back off the selected lines, or the caret's line
    /// (Shift+Tab). A line with no indentation is left exactly as it is.
    ///
    /// A line with *less* than a full level gives back what it has rather than
    /// refusing: outdent's job is to walk a line left, and real documents — hand
    /// written, or reflowed by some other editor — are full of indentation that
    /// was never a clean multiple of anything. Refusing there would strand the
    /// line at a depth Shift+Tab couldn't undo.
    pub fn outdent(&mut self) {
        if self.verse_reindent(false) {
            return;
        }
        self.reindent(false);
        self.renumber_here();
    }

    /// The body of [`indent`](Self::indent) / [`outdent`](Self::outdent).
    ///
    /// One splice across the whole line range, never one per line: a Tab is one
    /// thing the user did, so it has to be one undo step and one reparse. Per
    /// line, twig would reparse the document once per line and leave a stack of
    /// steps that Shift+⌘Z walks back one line at a time.
    fn reindent(&mut self, add: bool) {
        let (sel_start, sel_end) = self.selection().unwrap_or((self.caret, self.caret));
        let start = source_line_range(&self.source, sel_start).start;
        let end = source_line_range(&self.source, sel_end).end;
        let region = self.source[start..end].to_string();
        let lines: Vec<&str> = region.split('\n').collect();
        // A blank line has no text to move, and padding it would leave nothing
        // but trailing whitespace — but Tab on a blank line *is* a request for
        // indentation to type into, so the skip only applies where the op has
        // other lines to do real work on.
        let skip_blank = add && lines.len() > 1;

        let mut out = String::with_capacity(region.len() + lines.len() * Self::INDENT.len());
        let mut deltas: Vec<isize> = Vec::with_capacity(lines.len());
        let mut line_off = start;
        for (i, full) in lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            // A list item moves by having its whole leading prefix *replaced*,
            // never by having spaces pushed in front of the line. twig spells
            // both prefixes, so the quote markers, the parent's indent and an
            // ordered marker's extra column all come out right without leaf
            // measuring any of them — and a line that only looks like an item
            // (a Djot continuation) reports no marker and is left to the plain
            // path, where a Tab is just a Tab.
            let marker = self.list_marker_on_line(line_off);
            let own = marker
                .as_ref()
                .map(|m| m.marker_start - m.line_start)
                .unwrap_or(0);
            let delta = if add {
                if skip_blank && full.trim().is_empty() {
                    out.push_str(full);
                    0
                } else if marker.is_some() && self.first_item_of_list(line_off) {
                    // The first item of a list has no preceding sibling to nest
                    // under, so a Tab here can't spell a sub-list — twig would
                    // reparse the shoved-over marker as the same list, only
                    // indented, which Shift+Tab then can't cleanly undo. Leave the
                    // item where it is, the way every list editor refuses to
                    // over-indent a list's first line.
                    out.push_str(full);
                    0
                } else if marker.is_some() {
                    // Nesting means standing where a *continuation* of this line
                    // would stand: past the parent's marker, inside its content
                    // column. That is `continuation_prefix`, less a checkbox.
                    let new = self.nesting_prefix_at(line_off);
                    let delta = new.len() as isize - own as isize;
                    out.push_str(&new);
                    out.push_str(&full[own..]);
                    delta
                } else {
                    out.push_str(Self::INDENT);
                    out.push_str(full);
                    Self::INDENT.len() as isize
                }
            } else if marker.is_some() {
                // Unnesting is the mirror: stand where the parent item's own
                // line starts, which drops exactly the level it contributed.
                let new = self.outdent_prefix_at(line_off);
                let delta = new.len() as isize - own as isize;
                out.push_str(&new);
                out.push_str(&full[own..]);
                delta
            } else {
                // A plain line gives back the ordinary step.
                let strip = outdent_width(full, Self::INDENT.len());
                out.push_str(&full[strip..]);
                -(strip as isize)
            };
            deltas.push(delta);
            line_off += full.len() + 1;
        }
        // Nothing to give back. Returning before the splice keeps an outdent at
        // column zero from spending an undo step on a document it never changed.
        if deltas.iter().all(|d| *d == 0) {
            return;
        }

        // Every line's text keeps its offset *within the line*, so the caret is
        // remapped by its column, not by its byte offset — which the prefixes on
        // the lines above it have already invalidated.
        let remap = |off: usize| -> usize {
            let (mut old_ls, mut new_ls) = (start, start);
            for (line, delta) in lines.iter().zip(&deltas) {
                let old_le = old_ls + line.len();
                let new_len = (line.len() as isize + delta) as usize;
                if off <= old_le {
                    let col = (off - old_ls) as isize;
                    return new_ls + ((col + delta).max(0) as usize).min(new_len);
                }
                old_ls = old_le + 1;
                new_ls += new_len + 1;
            }
            start + out.len()
        };
        let placed = match self.selection() {
            // Keep the rewritten region selected, the way a container toggle
            // keeps its own: it leaves a second Tab aimed at the same lines
            // rather than at whatever the shifted offsets now happen to cover.
            Some(_) => (start + out.len(), Some(start)),
            None => (remap(self.caret), None),
        };

        // A rolled-back splice leaves the old source in place, where every offset
        // computed above addresses text that was never written.
        if !self.splice(start, end, &out, EditKind::Other) {
            return;
        }
        // `splice` re-anchors to the end of the `Change`, which for a whole-region
        // rewrite is the last line's end — nowhere the caret was. Place it, then
        // re-record the caret so this is the state redo restores, not the one
        // `splice` left behind from the `Change`.
        self.caret = placed.0.min(self.source.len());
        self.anchor = placed.1;
        self.clamp_caret();
        self.record_caret();
    }
}
