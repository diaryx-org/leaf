//! What one frame's rows are as a change from the frame before's.
//!
//! A binding answers every gesture with a frame — the rows to paint, and
//! where the caret is among them — and lifting a whole frame across a
//! boundary (UniFFI records into Swift, serde into JS objects) costs the
//! document, not the gesture: on a long document the lift is most of a
//! keystroke and all of a click. Both renderers already keep the rows a frame
//! did not change, so what is left to make cheap is the crossing, and the way
//! to do that is to cross only the rows that changed.
//!
//! A binding does not need to know *why* rows changed to know *which* did.
//! It keeps the rows it last handed out and takes the common prefix and
//! suffix against the ones it is about to: comparing a few hundred rows in
//! memory is microseconds, and what falls between is exactly the span a
//! renderer diffing whole frames would have found for itself. The one wrinkle
//! is that a row carries the source offset each of its runs came from, and a
//! keystroke moves every offset after it — so read by `==`, every row below
//! an edit is a different row, and the "span" is half the document. A row
//! after the edit is the same row *moved*, and the suffix is matched on that
//! footing: equal in everything, with its offsets shifted by one constant,
//! which the frame then carries so the renderer moves its own copies the same
//! way. The shift is read off the rows and checked on every row it is claimed
//! for, never assumed from the edit, so a frame reproduces the rows exactly
//! or names the row as changed.
//!
//! This module is the algorithm; each binding supplies what "the same row,
//! shifted" means for its own row type, and the fields the frame carries the
//! answer in are documented on the binding's `DocView`.

/// Where two frames' rows differ: the span of the old rows a span of the new
/// ones replaces, and the offset the rows after it moved by. See
/// [`row_delta`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowDelta {
    /// The first row that differs — the length of the common prefix, which is
    /// where the new span begins in both the old rows and the new.
    pub start: usize,
    /// How many of the old rows, from `start`, the new span replaces.
    pub replaced: usize,
    /// How many new rows stand in their place.
    pub len: usize,
    /// The byte offset every source offset in a row after the span moved by:
    /// the rows after the span are the old ones with this added to each
    /// offset they carry. Zero when nothing moved, and when no row after the
    /// span carries an offset to move.
    pub src_shift: i64,
}

impl RowDelta {
    /// Whether the two frames' rows are the same rows.
    pub fn is_empty(&self) -> bool {
        self.replaced == 0 && self.len == 0
    }
}

/// The change from `old` to `new`, as the span outside their common prefix
/// and suffix.
///
/// `same(a, b, shift)` says whether `b` is `a` with every source offset it
/// carries moved by `shift` — `shift == 0` is plain equality. `shift_between(a,
/// b)` reads the shift `b`'s offsets stand at from `a`'s off any one offset the
/// two share, or `None` when `a` carries none (a blank row), which leaves the
/// shift to be read off a row that does.
///
/// The prefix is matched at shift zero: nothing before an edit moves. The
/// suffix is matched from the end, at whichever shift the first offset-bearing
/// row from the end stands at, and a row that does not stand at it — or that
/// differs in anything else — ends the suffix and is part of the span. So the
/// rows the delta reports as kept are exactly reproducible from the old rows
/// and `src_shift`, whatever the edit was.
pub fn row_delta<R>(
    old: &[R],
    new: &[R],
    same: impl Fn(&R, &R, i64) -> bool,
    shift_between: impl Fn(&R, &R) -> Option<i64>,
) -> RowDelta {
    let shortest = old.len().min(new.len());
    let mut start = 0;
    while start < shortest && same(&old[start], &new[start], 0) {
        start += 1;
    }
    let mut suffix = 0;
    let mut shift: Option<i64> = None;
    while suffix < shortest - start {
        let a = &old[old.len() - 1 - suffix];
        let b = &new[new.len() - 1 - suffix];
        let candidate = shift.or_else(|| shift_between(a, b));
        if !same(a, b, candidate.unwrap_or(0)) {
            break;
        }
        shift = candidate;
        suffix += 1;
    }
    RowDelta {
        start,
        replaced: old.len() - start - suffix,
        len: new.len() - start - suffix,
        src_shift: shift.unwrap_or(0),
    }
}

/// Apply `delta` — with the rows `span` it stands for — to `rows`, the frame
/// before's, so they become the frame's: the reference for what a renderer
/// does with the fields, and what the bindings' tests apply a sequence of
/// frames with. `shift(row, by)` moves every offset the row carries.
pub fn apply_row_delta<R: Clone>(
    rows: &mut Vec<R>,
    delta: RowDelta,
    span: &[R],
    shift: impl Fn(&mut R, i64),
) {
    let end = delta.start + delta.replaced;
    rows.splice(delta.start..end, span.iter().cloned());
    if delta.src_shift != 0 {
        for row in &mut rows[delta.start + delta.len..] {
            shift(row, delta.src_shift);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row for the tests: its text, and the offset it came from.
    #[derive(Clone, Debug, PartialEq)]
    struct R(&'static str, Option<i64>);

    fn same(a: &R, b: &R, shift: i64) -> bool {
        a.0 == b.0 && a.1.map(|s| s + shift) == b.1
    }
    fn between(a: &R, b: &R) -> Option<i64> {
        Some(b.1? - a.1?)
    }
    fn shift(r: &mut R, by: i64) {
        if let Some(s) = &mut r.1 {
            *s += by;
        }
    }
    fn delta(old: &[R], new: &[R]) -> RowDelta {
        let d = row_delta(old, new, same, between);
        // Whatever the answer, applying it reproduces the new rows.
        let mut applied = old.to_vec();
        apply_row_delta(&mut applied, d, &new[d.start..d.start + d.len], shift);
        assert_eq!(applied, new, "{d:?}");
        d
    }

    #[test]
    fn the_same_rows_are_an_empty_span() {
        let rows = [R("a", Some(0)), R("b", Some(2))];
        let d = delta(&rows, &rows);
        assert!(d.is_empty());
        assert_eq!(d.start, 2);
        assert_eq!(d.src_shift, 0);
    }

    #[test]
    fn a_keystroke_is_one_row_and_a_shift_for_the_rest() {
        let old = [R("a", Some(0)), R("b", Some(2)), R("c", Some(4))];
        let new = [R("a", Some(0)), R("bx", Some(2)), R("c", Some(5))];
        let d = delta(&old, &new);
        assert_eq!(
            d,
            RowDelta {
                start: 1,
                replaced: 1,
                len: 1,
                src_shift: 1
            }
        );
    }

    #[test]
    fn a_row_that_does_not_stand_at_the_shift_is_in_the_span() {
        // The last row moved by two, the one before it by one: only the last
        // is kept, whatever the middle one says.
        let old = [R("a", Some(0)), R("b", Some(2)), R("c", Some(4))];
        let new = [R("a", Some(0)), R("b", Some(3)), R("c", Some(6))];
        let d = delta(&old, &new);
        assert_eq!(d.start, 1);
        assert_eq!(d.replaced, 1);
        assert_eq!(d.src_shift, 2);
    }

    #[test]
    fn a_blank_row_at_the_end_leaves_the_shift_to_the_row_that_carries_one() {
        let old = [R("a", Some(0)), R("b", Some(2)), R("", None)];
        let new = [R("ab", Some(0)), R("b", Some(3)), R("", None)];
        let d = delta(&old, &new);
        assert_eq!((d.start, d.replaced, d.len, d.src_shift), (0, 1, 1, 1));
    }

    #[test]
    fn an_inserted_row_and_a_removed_one() {
        let a = R("a", Some(0));
        let b = R("b", Some(2));
        let c = R("c", Some(4));
        let d = delta(
            &[a.clone(), c.clone()],
            &[a.clone(), b.clone(), R("c", Some(6))],
        );
        assert_eq!((d.start, d.replaced, d.len, d.src_shift), (1, 0, 1, 2));
        let d = delta(&[a.clone(), b, c], &[a, R("c", Some(2))]);
        assert_eq!((d.start, d.replaced, d.len, d.src_shift), (1, 1, 0, -2));
    }

    #[test]
    fn everything_changed_is_the_whole_of_both() {
        let d = delta(&[R("a", Some(0))], &[R("x", Some(0)), R("y", Some(2))]);
        assert_eq!((d.start, d.replaced, d.len), (0, 1, 2));
        let d = delta(&[], &[R("x", Some(0))]);
        assert_eq!((d.start, d.replaced, d.len), (0, 0, 1));
        let d = delta(&[R("x", Some(0))], &[]);
        assert_eq!((d.start, d.replaced, d.len), (0, 1, 0));
    }
}
