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
//!
//! # What counts as a match
//!
//! Matching over the source finds text the *rich* view doesn't draw: the
//! `title:` inside hidden frontmatter, the URL inside `[text](url)`, the `**`
//! around a bold run. Offering those as matches is worse than not finding them.
//! They select nothing a reader can see, stepping to one looks like the bar
//! froze, and replacing one edits bytes the person doing it was never shown.
//!
//! So the policy is one line: **in the WYSIWYG view a match counts only if the
//! view draws some part of it** ([`is_visible`]). The rest are counted and said
//! out loud — "2 of 3, 1 hidden" — and are never stepped to, never washed, and
//! never replaced. The source view draws every byte, so nothing is hidden there
//! and the count is the whole count; searching the source view is how you reach
//! a hit in the frontmatter, which is also where you would have to be to edit it
//! by hand.
//!
//! "Draws some part of it" rather than "draws all of it", so a hit that runs
//! from visible text into a hidden delimiter is still a hit. The question is
//! asked of [`leaf_core::VisualMap`] as the last frame laid it out; `main`
//! rebuilds the map before asking, so the answer is never a frame behind the
//! bytes.

use leaf_core::{Doc, Highlight, View};

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
    /// Every match the view draws, in document order, as `[start, end)` source
    /// byte ranges. See the module docs for what "draws" means.
    pub matches: Vec<(usize, usize)>,
    /// How many matches the view does *not* draw. Counted so the caption can
    /// say so — a query that matches only hidden text has to read as "found,
    /// not shown" rather than as "not found".
    pub hidden: usize,
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
        let seeded = !query.is_empty();
        Find {
            query,
            query_cursor,
            replacement: replacing.then(String::new),
            replacement_cursor: 0,
            // The query, unless there is already one — there is nothing to
            // replace until there is something to find, but when ^h is pressed
            // over a selection the finding is already done and the next thing
            // anybody types is the replacement.
            field: if replacing && seeded {
                FindField::Replacement
            } else {
                FindField::Query
            },
            matches: Vec::new(),
            hidden: 0,
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
    pub fn set_matches(&mut self, matches: Vec<(usize, usize)>, hidden: usize, anchor: usize) {
        self.current = matches
            .iter()
            .position(|(start, _)| *start >= anchor)
            .unwrap_or(0);
        self.matches = matches;
        self.hidden = hidden;
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
    /// The `id` is empty on every one of them. It is the host's name for a
    /// range, handed back when a reader activates it — and the only host here is
    /// this bar, which finds its match by index and never looks one up by name.
    /// A `format!("find:{i}")` per hit was an allocation per hit for a string
    /// nothing read.
    pub fn highlights(&self) -> Vec<Highlight> {
        let mut out = Vec::with_capacity(self.matches.len());
        out.extend(self.matches.iter().map(|&(start, end)| Highlight {
            start,
            end,
            id: String::new(),
            color: None,
            marker: None,
        }));
        out
    }

    /// What the right-hand end of the bar says: which match of how many, or
    /// that there are none, or nothing at all before anything has been typed.
    pub fn caption(&self) -> String {
        let hidden = match self.hidden {
            0 => String::new(),
            n => format!(", {n} hidden"),
        };
        if self.query.is_empty() {
            String::new()
        } else if self.matches.is_empty() && self.hidden > 0 {
            // Found, but not anywhere this view draws — which is a different
            // answer from "not in this document", and has a different next step
            // (⌥w, into the source view).
            format!("none shown{hidden}")
        } else if self.matches.is_empty() {
            "no matches".to_string()
        } else {
            format!("{} of {}{hidden}", self.current + 1, self.matches.len())
        }
    }
}

/// Whether the active view draws any part of the source range `[start, end)` —
/// the whole of the policy in this module's docs.
///
/// In the source view every byte is on screen, so the answer is always yes. In
/// WYSIWYG it is asked of the caret stops: a stop is a place the rendered grid
/// has a home for a source offset, so a range with a stop inside it has
/// something drawn in it, and a range with none — hidden frontmatter, a link's
/// destination, the `**` around a bold run — has nothing.
pub fn is_visible(doc: &Doc, start: usize, end: usize) -> bool {
    match doc.view {
        View::Source => true,
        View::Wysiwyg => doc
            .vmap
            .stop_at_or_after(start)
            .is_some_and(|stop| stop < end),
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
/// Naive, at `O(n·m)` comparisons. A document held open in a terminal is small
/// enough that this is invisible next to the reparse a single keystroke already
/// costs, and the version that is obviously correct is worth more here than the
/// version that is fast.
///
/// The haystack is folded *once*, into `(source offset, folded char)`, and the
/// needle is then slid along that. Folding inside the candidate loop — which is
/// what this did — re-folded the same characters once per needle character, so
/// a six-letter query folded the whole document six times per keystroke. The
/// pairs keep the source offsets, which is the reason the whole thing is written
/// this way rather than over `to_lowercase`.
pub fn find_matches(source: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = needle.chars().map(fold_case).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let hay: Vec<(usize, char)> = source
        .char_indices()
        .map(|(at, c)| (at, fold_case(c)))
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if (0..needle.len()).all(|k| hay[i + k].1 == needle[k]) {
            // The end is the *next* character's offset (or the end of the
            // source), never a folded char's own length: folding can change how
            // many bytes a character takes, and these are offsets into `source`.
            let end = hay
                .get(i + needle.len())
                .map_or(source.len(), |&(at, _)| at);
            out.push((hay[i].0, end));
            i += needle.len();
        } else {
            i += 1;
        }
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

        find.set_matches(matches.clone(), 0, 0);
        assert_eq!(find.current, 0);
        find.set_matches(matches.clone(), 0, 5);
        assert_eq!(find.current, 1, "the first hit at or after the anchor");
        // Past the last one, the search comes round again rather than going
        // quiet at the end of the document.
        find.set_matches(matches, 0, 999);
        assert_eq!(find.current, 0);
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        let mut find = Find::new(false, "one".into());
        find.set_matches(find_matches("one one one", "one"), 0, 0);
        assert_eq!(find.step(-1), find.matches.last().copied(), "back off zero");
        assert_eq!(find.step(1), find.matches.first().copied(), "and forward");
    }

    #[test]
    fn the_caption_counts_from_one_and_says_when_there_is_nothing() {
        let mut find = Find::new(false, String::new());
        assert_eq!(find.caption(), "", "nothing typed yet is not 'no matches'");
        find.query = "zzz".into();
        assert_eq!(find.caption(), "no matches");
        find.set_matches(find_matches("one one", "one"), 0, 0);
        assert_eq!(find.caption(), "1 of 2");
        find.step(1);
        assert_eq!(find.caption(), "2 of 2");
    }

    /// The caption is where the visibility policy is said out loud: a hit the
    /// rich view doesn't draw is neither offered nor silently dropped.
    #[test]
    fn the_caption_says_how_many_matches_the_view_is_not_showing() {
        let mut find = Find::new(false, "one".into());
        find.set_matches(find_matches("one one", "one"), 1, 0);
        assert_eq!(find.caption(), "1 of 2, 1 hidden");
        // Matched, but nowhere this view draws — a different answer from "not
        // in this document", and with a different next step (⌥w).
        find.set_matches(Vec::new(), 2, 0);
        assert_eq!(find.caption(), "none shown, 2 hidden");
        find.set_matches(Vec::new(), 0, 0);
        assert_eq!(find.caption(), "no matches");
    }

    #[test]
    fn opening_to_replace_over_a_selection_puts_the_keyboard_in_the_replacement() {
        // ^h with nothing selected: the query first, because there is nothing to
        // replace until there is something to find.
        assert_eq!(Find::new(true, String::new()).field, FindField::Query);
        // ^h over a selection: the finding is already done.
        assert_eq!(Find::new(true, "seed".into()).field, FindField::Replacement);
        // A plain ^f never lands in a field it doesn't have.
        assert_eq!(Find::new(false, "seed".into()).field, FindField::Query);
    }

    #[test]
    fn a_plain_find_grows_no_replacement_field() {
        assert!(Find::new(false, String::new()).replacement.is_none());
        assert_eq!(Find::new(false, String::new()).rows(), 2);
        assert_eq!(Find::new(true, String::new()).rows(), 3);
    }
}
