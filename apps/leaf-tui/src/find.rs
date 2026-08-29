//! The find bar — what ^f and ^h put up, and the search behind it.
//!
//! Shaped like [`crate::palette::Palette`]: the state lives here and on `App`,
//! `main` drives it from the keyboard, and `ui` paints it. What it is *not* is
//! another centered modal. A search is read against the document it is
//! searching, so a panel over the middle of the page would cover the thing
//! being looked for; the bar is a footer strip instead, and it takes its rows
//! out of the editing surface rather than floating over them.
//!
//! # Matching
//!
//! Over `doc.source`, not over anything rendered, and case-insensitively.
//!
//! Source is the right side of that seam even though the rich view hides
//! delimiters, because the offsets a match produces are the same offsets the
//! caret, the selection, and [`leaf_core::Highlight`] are already in — so a hit
//! inside `**bold**` selects and scrolls with no coordinate conversion at all,
//! and a hit that lands *on* a hidden `**` still resolves to a real caret stop
//! through `Doc::place_caret`. Searching the rendered text instead would mean
//! mapping back through the visual map for every one of those, and would make
//! `# ` unfindable in a document that contains it.

use leaf_core::Highlight;

/// Which of the bar's two fields the keyboard is typing into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindField {
    Query,
    Replacement,
}

/// The find bar's state while it's open.
pub struct Find {
    /// The needle. Empty matches nothing — deliberately, since every offset in
    /// the document matches an empty string and painting the whole file is not
    /// an answer to anything.
    pub query: String,
    /// Byte offset into `query`, only ever moved by whole `char`s.
    pub query_cursor: usize,
    /// The replacement text — `Some` only when the bar was opened to replace
    /// (^h), which is also what decides whether the second row is drawn. A
    /// plain ^f find never grows one by accident.
    pub replacement: Option<String>,
    /// Byte offset into `replacement`, as `query_cursor` is into `query`.
    pub replacement_cursor: usize,
    /// Which field has the keyboard, and the terminal cursor.
    pub field: FindField,
    /// Every match, in document order, as `[start, end)` source byte ranges.
    pub matches: Vec<(usize, usize)>,
    /// Index into `matches` of the one the caret is on. Meaningless — and never
    /// read — when `matches` is empty.
    pub current: usize,
    /// Where "the current match" is measured from while the query is being
    /// typed: the caret as of the last thing that deliberately moved it (the
    /// bar opening, a step, a replacement).
    ///
    /// Anchored rather than read live off the caret because the search jumps
    /// the caret to what it found on every keystroke — so measuring from the
    /// caret would make the highlight crawl forward under the typist's own
    /// jumps, and `hel` would find a different `hello` than `hell` did.
    pub origin: usize,
}

impl Find {
    /// Open the bar, seeded with `query` — the selection, usually, since the
    /// commonest search is for the thing already under the caret.
    pub fn new(replacing: bool, query: String) -> Self {
        let query_cursor = query.len();
        Find {
            query,
            query_cursor,
            replacement: replacing.then(String::new),
            replacement_cursor: 0,
            // The query, even when opened to replace: there is nothing to
            // replace until there is something to find.
            field: FindField::Query,
            matches: Vec::new(),
            current: 0,
            origin: 0,
        }
    }

    /// How many terminal rows the bar draws in: the query, the replacement when
    /// there is one, and the key hints under them.
    pub fn rows(&self) -> u16 {
        if self.replacement.is_some() { 3 } else { 2 }
    }

    /// The focused field's text and cursor, for typing into.
    pub fn field_mut(&mut self) -> (&mut String, &mut usize) {
        match self.field {
            FindField::Query => (&mut self.query, &mut self.query_cursor),
            FindField::Replacement => (
                self.replacement.get_or_insert_default(),
                &mut self.replacement_cursor,
            ),
        }
    }

    /// The focused field's text and cursor, for drawing.
    pub fn field(&self) -> (&str, usize) {
        match self.field {
            FindField::Query => (&self.query, self.query_cursor),
            FindField::Replacement => (
                self.replacement.as_deref().unwrap_or(""),
                self.replacement_cursor,
            ),
        }
    }

    /// Take a fresh set of matches and pick which one is current: the first
    /// starting at or after `anchor`, wrapping to the first in the document
    /// when the anchor is past them all. Wrapping rather than stopping, because
    /// a search that goes quiet at the end of the file reads as a search that
    /// found nothing.
    pub fn set_matches(&mut self, matches: Vec<(usize, usize)>, anchor: usize) {
        self.current = matches
            .iter()
            .position(|(start, _)| *start >= anchor)
            .unwrap_or(0);
        self.matches = matches;
    }

    /// The match the caret is on, if there is one.
    pub fn current_match(&self) -> Option<(usize, usize)> {
        self.matches.get(self.current).copied()
    }

    /// Step to the next (`1`) or previous (`-1`) match, wrapping, and return
    /// where it is. `None` when there is nothing to step through.
    pub fn step(&mut self, delta: isize) -> Option<(usize, usize)> {
        let n = self.matches.len() as isize;
        if n == 0 {
            return None;
        }
        self.current = (self.current as isize + delta).rem_euclid(n) as usize;
        self.current_match()
    }

    /// The matches as ranges for [`leaf_core::Doc::set_highlights`] to paint.
    ///
    /// All of them get the theme's default wash and no color of their own: the
    /// *current* one is told apart by being the selection, which the renderer
    /// draws reversed on top of the wash. Two washes would have meant picking a
    /// second color here, in the host, against a terminal palette it can't see.
    pub fn highlights(&self) -> Vec<Highlight> {
        self.matches
            .iter()
            .enumerate()
            .map(|(i, (start, end))| Highlight {
                start: *start,
                end: *end,
                id: format!("find:{i}"),
                color: None,
                marker: None,
            })
            .collect()
    }

    /// What the right-hand end of the bar says: which match of how many, or
    /// that there are none, or nothing at all before anything has been typed.
    pub fn caption(&self) -> String {
        if self.query.is_empty() {
            String::new()
        } else if self.matches.is_empty() {
            "no matches".to_string()
        } else {
            format!("{} of {}", self.current + 1, self.matches.len())
        }
    }
}

/// Every non-overlapping, case-insensitive occurrence of `needle` in `source`,
/// as `[start, end)` byte ranges in document order.
///
/// Non-overlapping because replace-all is the other caller and overlapping
/// matches cannot all be replaced; `aa` in `aaa` is one hit, not two.
///
/// The comparison folds one char to one char (see [`fold_case`]) rather than
/// going through `str::to_lowercase`, so that the ranges are offsets into
/// `source` itself. Lowercasing the haystack first is the obvious
/// implementation and quietly the wrong one: `İ` lowercases to *two* chars, so
/// every offset after one of those in the document would be shifted, and the
/// match would be painted somewhere other than where it is.
///
/// Naive, at `O(n·m)`. A document held open in a terminal is small enough that
/// this is invisible next to the reparse a single keystroke already costs, and
/// the version that is obviously correct is worth more here than the version
/// that is fast.
pub fn find_matches(source: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = needle.chars().map(fold_case).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = 0;
    'candidate: while at < source.len() {
        let mut chars = source[at..].char_indices();
        let mut end = at;
        for want in &needle {
            match chars.next() {
                Some((offset, c)) if fold_case(c) == *want => end = at + offset + c.len_utf8(),
                // No match here (or the source ran out): step one whole char
                // along and try again from there.
                _ => {
                    at += source[at..].chars().next().map_or(1, char::len_utf8);
                    continue 'candidate;
                }
            }
        }
        out.push((at, end));
        at = end;
    }
    out
}

/// One char's lowercase form, where it has a single-char one.
///
/// `char::to_lowercase` yields an iterator because a few characters lowercase
/// to several — `İ` to `i̇`, and `ß`/`SS` in the other direction. Taking the
/// first is a simple case fold: it gets `É`/`é`, `Ä`/`ä`, Cyrillic and Greek
/// right, which is what "case-insensitive" means to anyone typing into a find
/// bar, and treats the handful of one-to-many pairs as distinct. That is a
/// limit worth having in exchange for byte offsets that are exactly where the
/// text is.
fn fold_case(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_ignores_case_in_both_directions() {
        assert_eq!(find_matches("Hello hello HELLO", "hello").len(), 3);
        assert_eq!(find_matches("Hello hello HELLO", "HeLLo").len(), 3);
    }

    #[test]
    fn matches_are_source_byte_ranges_that_slice_back_to_the_text() {
        let source = "one two one";
        let hits = find_matches(source, "ONE");
        assert_eq!(hits, vec![(0, 3), (8, 11)]);
        for (s, e) in hits {
            assert_eq!(&source[s..e], "one");
        }
    }

    /// The reason the haystack isn't lowercased first: a multi-byte char before
    /// a match must not shift the offsets it is reported at.
    #[test]
    fn offsets_survive_multibyte_text_ahead_of_the_match() {
        let source = "你好 café Über hello";
        let (s, e) = find_matches(source, "hello")[0];
        assert_eq!(&source[s..e], "hello");
        // …and a non-ASCII needle folds too.
        let (s, e) = find_matches(source, "ÜBER")[0];
        assert_eq!(&source[s..e], "Über");
        let (s, e) = find_matches(source, "CAFÉ")[0];
        assert_eq!(&source[s..e], "café");
    }

    #[test]
    fn overlapping_candidates_are_reported_once_each() {
        // Replace-all is the other caller, and two overlapping hits cannot both
        // be replaced — so `aa` in `aaa` is one match, not two.
        assert_eq!(find_matches("aaa", "aa"), vec![(0, 2)]);
        assert_eq!(find_matches("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn an_empty_needle_matches_nothing() {
        assert!(find_matches("anything at all", "").is_empty());
    }

    #[test]
    fn a_needle_longer_than_what_is_left_does_not_run_off_the_end() {
        assert!(find_matches("short", "much longer needle").is_empty());
        assert_eq!(find_matches("abcabc", "abcabc"), vec![(0, 6)]);
    }

    #[test]
    fn the_current_match_is_the_first_at_or_after_the_anchor_and_wraps() {
        let mut find = Find::new(false, "one".into());
        let matches = find_matches("one two one two one", "one");
        assert_eq!(matches.len(), 3);

        find.set_matches(matches.clone(), 0);
        assert_eq!(find.current, 0);
        find.set_matches(matches.clone(), 5);
        assert_eq!(find.current, 1, "the first hit at or after the anchor");
        // Past the last one, the search comes round again rather than going
        // quiet at the end of the document.
        find.set_matches(matches, 999);
        assert_eq!(find.current, 0);
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        let mut find = Find::new(false, "one".into());
        find.set_matches(find_matches("one one one", "one"), 0);
        assert_eq!(find.step(-1), find.matches.last().copied(), "back off zero");
        assert_eq!(find.step(1), find.matches.first().copied(), "and forward");
    }

    #[test]
    fn the_caption_counts_from_one_and_says_when_there_is_nothing() {
        let mut find = Find::new(false, String::new());
        assert_eq!(find.caption(), "", "nothing typed yet is not 'no matches'");
        find.query = "zzz".into();
        assert_eq!(find.caption(), "no matches");
        find.set_matches(find_matches("one one", "one"), 0);
        assert_eq!(find.caption(), "1 of 2");
        find.step(1);
        assert_eq!(find.caption(), "2 of 2");
    }

    #[test]
    fn a_plain_find_grows_no_replacement_field() {
        assert!(Find::new(false, String::new()).replacement.is_none());
        assert_eq!(Find::new(false, String::new()).rows(), 2);
        assert_eq!(Find::new(true, String::new()).rows(), 3);
    }
}
