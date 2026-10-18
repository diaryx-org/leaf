//! Which rows a frame changed, and the handles the renderer hands Dioxus.
//!
//! The virtual DOM will find the change by itself — that is what a virtual DOM
//! is for. The point of this module is that finding it *there* costs the
//! document: Dioxus has to render every row's `rsx!` and walk every run to
//! discover that all but one are the same. leaf already knows which rows
//! changed, from [`leaf_core::row_delta`], and can say so in a form the diff
//! understands: the same [`RowHandle`] `Rc`, which memoizes the row's
//! component away entirely.
//!
//! So this is the bridge between leaf's own frame model and Dioxus's. It keeps
//! one `u64` per row — a **fingerprint** of what that row draws — and diffs
//! those. A fingerprint rather than a copy of the rows because a copy means an
//! allocation per row per keystroke, which is the cost being avoided; and
//! because it falls out of the fingerprint that the comparison is *visual*:
//! two rows with the same text and styles at different source offsets have the
//! same fingerprint, so a keystroke does not invalidate every row beneath it.
//! That is the same reason the runs carry no `data-src` (see [`crate::bridge::rows`]),
//! and it is why `RowDelta::src_shift` never comes up here.

use std::hash::{DefaultHasher, Hash, Hasher};

use leaf_core::{VRow, VisualMap, row_delta};

use crate::bridge::rows::{Marks, Opts, RowHandle, row_view};
use crate::bridge::style::class_of;

/// The renderer's own copy of the frame: a handle and a fingerprint per row.
#[derive(Default)]
pub struct Frame {
    /// How rows are built — see [`Opts::offsets`].
    opts: Opts,
    /// The handles, in row order — what the component renders.
    handles: Vec<RowHandle>,
    /// One fingerprint per row, parallel to `handles`.
    prints: Vec<u64>,
    /// The rows the caret or selection decorated last frame, so they can be
    /// rebuilt when it moves off them.
    marked: Vec<usize>,
    /// What the last [`update`](Self::update) rebuilt, for the bench to report.
    pub last: Stats,
}

/// What one frame cost this side of Dioxus. The editor reads `rebuilt`; the
/// rest say why, for the tests.
#[derive(Clone, Copy, Default, Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the tests read why a row was rebuilt")
)]
pub struct Stats {
    /// Rows whose handle is new — the ones Dioxus will actually render.
    pub rebuilt: usize,
    /// Rows in the document.
    pub total: usize,
    /// Rows the text delta replaced.
    pub replaced: usize,
    /// Rows rebuilt only because the caret or selection moved on or off them.
    pub remarked: usize,
}

impl Frame {
    /// Bring the frame up to `vmap` and `marks`, and answer with the rows to
    /// render. Every row the document did not change keeps the exact `Rc` it
    /// had, so its component memoizes.
    pub fn update(&mut self, vmap: &VisualMap, marks: &Marks) -> &[RowHandle] {
        let mut buf = String::new();
        let prints: Vec<u64> = vmap
            .rows
            .iter()
            .map(|r| fingerprint(r, &mut buf, self.opts))
            .collect();

        // Offset-blind, so the comparison is of what the rows draw: `same`
        // ignores the shift it is passed, and `shift_between` declines to
        // propose one, which leaves `src_shift` at zero by construction.
        let delta = row_delta(&self.prints, &prints, |a, b, _| a == b, |_, _| None);

        let span: Vec<RowHandle> = (delta.start..delta.start + delta.len)
            .map(|r| RowHandle::new(row_view(&vmap.rows[r], r, marks, self.opts)))
            .collect();
        self.handles
            .splice(delta.start..delta.start + delta.replaced, span);
        self.prints = prints;

        // Rows the marks left or arrived on. Their text is unchanged, so the
        // delta above says nothing about them, but what they draw did change.
        let marked = marked_rows(vmap, marks);
        let drift = delta.len as isize - delta.replaced as isize;
        let mut remarked = 0;
        let was = std::mem::take(&mut self.marked);
        let moved = was.into_iter().filter_map(|i| {
            if i < delta.start {
                Some(i)
            } else if i >= delta.start + delta.replaced {
                Some((i as isize + drift) as usize)
            } else {
                // Inside the span: already rebuilt.
                None
            }
        });
        let mut candidates: Vec<usize> = moved.chain(marked.iter().copied()).collect();
        candidates.sort_unstable();
        candidates.dedup();
        for row in candidates {
            let inside = row >= delta.start && row < delta.start + delta.len;
            if inside || row >= self.handles.len() {
                continue;
            }
            // Built and *compared* rather than assigned: a row the caret was
            // on and is still on at the same column draws what it drew, and
            // renewing its handle would cost a render for nothing. The
            // comparison is by value, but only ever over the handful of rows
            // the marks touch.
            let view = row_view(&vmap.rows[row], row, marks, self.opts);
            if *self.handles[row].0 != view {
                self.handles[row] = RowHandle::new(view);
                remarked += 1;
            }
        }
        self.marked = marked;

        self.last = Stats {
            rebuilt: delta.len + remarked,
            total: vmap.rows.len(),
            replaced: delta.replaced,
            remarked,
        };
        &self.handles
    }

    /// The rows as they stand, without an update.
    #[cfg(test)]
    pub fn handles(&self) -> &[RowHandle] {
        &self.handles
    }
}

/// The rows the caret, the selection, a composition, the misspellings and
/// the host's highlights decorate.
fn marked_rows(vmap: &VisualMap, marks: &Marks) -> Vec<usize> {
    let mut rows = Vec::new();
    if let Some((r, _)) = marks.caret {
        rows.push(r);
    }
    if let Some((a, b)) = marks.grips {
        rows.extend([a.0, b.0]);
    }
    let ranges = [marks.selection, marks.composing].into_iter().flatten();
    let highlights = marks.highlights.iter().map(|h| (h.start, h.end));
    for (lo, hi) in ranges
        .chain(marks.misspelled.iter().copied())
        .chain(highlights)
    {
        let (first, last) = vmap.row_range_for(lo..hi);
        rows.extend(first..=last.min(vmap.rows.len().saturating_sub(1)));
    }
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// A hash of everything about a row that reaches the DOM — and of nothing
/// else. The source offsets are left out deliberately: see the module note.
///
/// The style goes in as the class string the renderer would write, so the
/// fingerprint cannot disagree with what is drawn, and so this needs no second
/// encoding of [`leaf_core::Style`] to be kept in step with the first. `buf` is
/// the caller's, reused across every row and glyph of the frame, so a
/// fingerprint allocates nothing.
fn fingerprint(row: &VRow, buf: &mut String, opts: Opts) -> u64 {
    let mut h = DefaultHasher::new();
    row.code.hash(&mut h);
    row.decoration.hash(&mut h);
    row.code_lang.hash(&mut h);
    let mut open: Option<leaf_core::Style> = None;
    for g in &row.glyphs {
        if open != Some(g.style) {
            buf.clear();
            class_of(&g.style, buf);
            buf.hash(&mut h);
            open = Some(g.style);
        }
        g.drawn().hash(&mut h);
        if opts.offsets {
            g.src.hash(&mut h);
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use leaf_core::{Doc, Format};

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.to_string(), Format::Markdown).expect("a document");
        d.build_visual(80);
        d
    }

    fn update(frame: &mut Frame, d: &mut Doc) -> Stats {
        d.build_visual(80);
        let marks = Marks::of(&d.vmap, d.caret, d.anchor);
        frame.update(&d.vmap, &marks);
        frame.last
    }

    #[test]
    fn the_first_frame_builds_every_row() {
        let mut d = doc("a\n\nb\n\nc\n");
        let mut f = Frame::default();
        let s = update(&mut f, &mut d);
        assert_eq!(s.rebuilt, s.total);
        assert!(s.total >= 3, "{s:?}");
    }

    #[test]
    fn an_unchanged_document_rebuilds_nothing() {
        let mut d = doc("a\n\nb\n\nc\n");
        let mut f = Frame::default();
        update(&mut f, &mut d);
        let before: Vec<_> = f.handles().to_vec();
        let s = update(&mut f, &mut d);
        assert_eq!(s.rebuilt, 0, "{s:?}");
        assert!(
            before.iter().zip(f.handles()).all(|(a, b)| a == b),
            "every handle is the same Rc"
        );
    }

    /// The claim the measurements rest on: a keystroke renews one row's
    /// handle, whatever the document's length, and leaves the rest identical.
    #[test]
    fn a_keystroke_renews_one_row() {
        let body = "Some paragraph of prose.\n\n".repeat(200);
        let mut d = doc(&body);
        let mut f = Frame::default();
        update(&mut f, &mut d);
        let before: Vec<_> = f.handles().to_vec();

        d.place_caret(5, false);
        update(&mut f, &mut d);
        let after_move = f.last;
        d.insert("x");
        let s = update(&mut f, &mut d);

        assert!(s.total > 300, "a long document: {s:?}");
        assert_eq!(s.rebuilt, 1, "one row changed: {s:?}");
        let same = before
            .iter()
            .zip(f.handles())
            .filter(|(a, b)| a == b)
            .count();
        assert_eq!(
            same,
            s.total - 1,
            "every other row kept its Rc (after a move that cost {after_move:?})"
        );
    }

    #[test]
    fn a_caret_move_renews_only_the_two_rows_it_touches() {
        let mut d = doc(&"Prose.\n\n".repeat(50));
        let mut f = Frame::default();
        d.place_caret(0, false);
        update(&mut f, &mut d);
        d.move_down(false);
        let s = update(&mut f, &mut d);
        assert_eq!(s.replaced, 0, "no text changed: {s:?}");
        assert!(s.remarked <= 2, "the row left and the row arrived: {s:?}");
    }

    /// Typing shifts every offset after the caret. A renderer that kept
    /// offsets in its rows would see every row below the edit as different;
    /// this one does not, which is what the `data-src`-free runs buy.
    #[test]
    fn rows_below_an_edit_are_untouched_though_their_offsets_moved() {
        let mut d = doc("first\n\nsecond\n\nthird\n");
        let mut f = Frame::default();
        d.place_caret(0, false);
        update(&mut f, &mut d);
        let last = f.handles().last().expect("a last row").clone();
        d.insert("X");
        update(&mut f, &mut d);
        assert_eq!(
            &last,
            f.handles().last().expect("a last row"),
            "the last row's offsets moved but its handle did not"
        );
    }
}
