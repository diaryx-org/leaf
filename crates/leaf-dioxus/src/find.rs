//! Find and replace — ⌘F, ⌘G, ⇧⌘G, ⌥⌘F — the Swift editor's find bar.
//!
//! Swift uses the system's `NSTextFinder` over the document's text; this is
//! the same behaviour drawn by the editor: an incremental search as the query
//! is typed, case-insensitive, wrapping at either end, a match count, and the
//! match shown as the selection. It searches the **source**, so a match is a
//! pair of source offsets that select and replace through core like any other
//! range, in either view.
//!
//! The query and replacement are fields of the editor's own, not Blitz's
//! `<input>`: Blitz's inputs depend on DOM focus, which the editor gave up on
//! for its keys (see `keys.rs`), and their keystrokes would reach the editor's
//! window-level key handler too. So the fields are a string and a caret,
//! edited by the same keys the document gets while one of them has focus.
//!
//! Case folding is ASCII-only, which keeps every byte offset where it was;
//! a non-ASCII letter matches only its own case.

use dioxus::html::{Key, Modifiers, NamedKey};
use leaf_core::Doc;

/// Where typing goes while the find bar is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Focus {
    #[default]
    Doc,
    Query,
    Replacement,
}

/// The find bar's state.
#[derive(Default, Debug)]
pub struct Find {
    pub open: bool,
    /// The replace row is showing (⌥⌘F).
    pub replacing: bool,
    pub focus: Focus,
    pub query: Field,
    pub replacement: Field,
    /// Where the search began — the selection's start or the caret when the
    /// bar opened — which an incremental search measures from, so typing
    /// `ro` then `row` stays on the same match.
    pub origin: usize,
    /// The matches for the current query, as of `revision`.
    matches: Vec<(usize, usize)>,
    searched: Option<(u64, String)>,
}

/// What a key did in the find bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The query changed: search again from where the search began.
    Searched,
    /// Go to the next or previous match.
    Step {
        forward: bool,
    },
    /// Replace the current match and go to the next; or every match.
    Replace,
    ReplaceAll,
    /// Close the bar; the selection stays on the match.
    Close,
    /// Handled within the bar, nothing for the document to do.
    Edited,
    /// ⌘V in a field: the clipboard's text, which the host holds, wanted in
    /// the focused field (through [`Find::commit`]).
    Paste,
    /// Not the bar's key.
    Ignored,
}

impl Find {
    /// Open the bar (⌘F, or ⌥⌘F with the replace row), with focus in the
    /// query. A selection on one line becomes the query, as in a Mac text
    /// view; otherwise the last query stays.
    pub fn show(&mut self, doc: &Doc, replacing: bool) {
        self.open = true;
        self.replacing |= replacing;
        self.focus = Focus::Query;
        self.origin = doc.selection().map_or(doc.caret, |(lo, _)| lo);
        if let Some(sel) = doc.selected_text().filter(|s| !s.contains('\n')) {
            self.query.set(sel);
        }
        self.query.end();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.replacing = false;
        self.focus = Focus::Doc;
    }

    /// The matches for the query in `doc`, searched again only when the
    /// query or the document has changed.
    pub fn matches(&mut self, doc: &Doc) -> &[(usize, usize)] {
        let key = (doc.revision(), self.query.text.clone());
        if self.searched.as_ref() != Some(&key) {
            self.matches = search(&doc.source, &self.query.text);
            self.searched = Some(key);
        }
        &self.matches
    }

    /// "3 of 12", "No results", or nothing for an empty query.
    pub fn count(&mut self, doc: &Doc) -> String {
        if self.query.text.is_empty() {
            return String::new();
        }
        let sel = doc.selection();
        let m = self.matches(doc);
        match (m.len(), sel.and_then(|s| m.iter().position(|&x| x == s))) {
            (0, _) => "No results".into(),
            (n, Some(i)) => format!("{} of {n}", i + 1),
            (n, None) => format!("{n} found"),
        }
    }

    /// A key while focus is in the bar.
    pub fn key(&mut self, key: &Key, mods: Modifiers) -> Outcome {
        let shift = mods.contains(Modifiers::SHIFT);
        let alt = mods.contains(Modifiers::ALT);
        match key {
            Key::Named(NamedKey::Escape) => return Outcome::Close,
            Key::Named(NamedKey::Enter) if self.focus == Focus::Replacement => {
                return if alt {
                    Outcome::ReplaceAll
                } else {
                    Outcome::Replace
                };
            }
            Key::Named(NamedKey::Enter) => return Outcome::Step { forward: !shift },
            Key::Named(NamedKey::Tab) if self.replacing => {
                self.focus = match self.focus {
                    Focus::Query => Focus::Replacement,
                    _ => Focus::Query,
                };
                return Outcome::Edited;
            }
            _ => {}
        }
        let query = self.focus == Focus::Query;
        let field = match self.focus {
            Focus::Query => &mut self.query,
            Focus::Replacement => &mut self.replacement,
            Focus::Doc => return Outcome::Ignored,
        };
        match field.key(key, mods) {
            FieldEdit::Changed if query => Outcome::Searched,
            FieldEdit::Changed | FieldEdit::Moved => Outcome::Edited,
            FieldEdit::Paste => Outcome::Paste,
            FieldEdit::Ignored => Outcome::Ignored,
        }
    }

    /// Text the input method committed — an accent, an emoji — into the
    /// focused field.
    pub fn commit(&mut self, text: &str) -> Outcome {
        match self.focus {
            Focus::Query => {
                self.query.insert(text);
                Outcome::Searched
            }
            Focus::Replacement => {
                self.replacement.insert(text);
                Outcome::Edited
            }
            Focus::Doc => Outcome::Ignored,
        }
    }

    /// The match to go to: the first at or after `from`, or the last before
    /// it going back, wrapping round at either end.
    pub fn step(&mut self, doc: &Doc, forward: bool) -> Option<(usize, usize)> {
        let (lo, hi) = doc.selection().unwrap_or((doc.caret, doc.caret));
        let m = self.matches(doc);
        if forward {
            m.iter().find(|&&(s, _)| s >= hi).or(m.first()).copied()
        } else {
            m.iter()
                .rev()
                .find(|&&(_, e)| e <= lo)
                .or(m.last())
                .copied()
        }
    }

    /// The first match at or after `from`, for an incremental search that
    /// stays put while the query still matches where it is.
    pub fn first_from(&mut self, doc: &Doc, from: usize) -> Option<(usize, usize)> {
        let m = self.matches(doc);
        m.iter().find(|&&(s, _)| s >= from).or(m.first()).copied()
    }
}

/// Replace the selection, if it is a match, with the replacement; answers
/// whether it did.
pub fn replace_current(find: &mut Find, doc: &mut Doc) -> bool {
    let Some(sel) = doc.selection() else {
        return false;
    };
    if !find.matches(doc).contains(&sel) {
        return false;
    }
    let with = find.replacement.text.clone();
    doc.edit(sel.0, sel.1, &with);
    true
}

/// Replace every match, as one undo step; answers how many.
pub fn replace_all(find: &mut Find, doc: &mut Doc) -> usize {
    let matches = find.matches(doc).to_vec();
    let with = find.replacement.text.clone();
    doc.begin_undo_group();
    // From the end, so each edit leaves the offsets before it standing.
    for &(s, e) in matches.iter().rev() {
        doc.edit(s, e, &with);
    }
    doc.end_undo_group();
    matches.len()
}

/// Every non-overlapping occurrence of `query` in `source`, ignoring ASCII
/// case.
pub fn search(source: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let hay = source.to_ascii_lowercase();
    let needle = query.to_ascii_lowercase();
    hay.match_indices(&needle)
        .map(|(i, m)| (i, i + m.len()))
        .collect()
}

/// What the find bar draws, copied out of [`Find`] for the render.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarView {
    pub open: bool,
    pub replacing: bool,
    pub focus: Focus,
    pub query: Field,
    pub replacement: Field,
    pub count: String,
}

impl BarView {
    pub fn of(find: &mut Find, doc: &Doc) -> Self {
        Self {
            open: find.open,
            replacing: find.replacing,
            focus: find.focus,
            query: find.query.clone(),
            replacement: find.replacement.clone(),
            count: if find.open {
                find.count(doc)
            } else {
                String::new()
            },
        }
    }

    /// How tall the bar draws, in CSS pixels: a row for find, and one more
    /// for replace.
    pub fn height(&self) -> f64 {
        match (self.open, self.replacing) {
            (false, _) => 0.0,
            (true, false) => ROW,
            (true, true) => 2.0 * ROW,
        }
    }
}

/// One row of the find bar.
pub const ROW: f64 = 34.0;

/// A one-line text field: the query, the replacement.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Field {
    pub text: String,
    /// The caret, in bytes.
    pub caret: usize,
}

enum FieldEdit {
    Changed,
    Moved,
    Paste,
    Ignored,
}

impl Field {
    fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.caret = self.text.len();
    }

    fn end(&mut self) {
        self.caret = self.text.len();
    }

    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    fn prev(&self) -> usize {
        self.text[..self.caret]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |c| self.caret + c.len_utf8())
    }

    fn key(&mut self, key: &Key, mods: Modifiers) -> FieldEdit {
        let cmd = mods.contains(Modifiers::META);
        match key {
            Key::Character(c) if cmd && c == "v" => FieldEdit::Paste,
            // ⌘A in a field clears it on the next keystroke in a Mac text
            // field; with no selection to draw, it clears now.
            Key::Character(c) if cmd && c == "a" => {
                self.text.clear();
                self.caret = 0;
                FieldEdit::Changed
            }
            Key::Character(_) if cmd => FieldEdit::Ignored,
            Key::Character(c) => {
                self.insert(c);
                FieldEdit::Changed
            }
            Key::Named(NamedKey::Backspace) if self.caret > 0 => {
                let at = if mods.contains(Modifiers::ALT) || cmd {
                    0
                } else {
                    self.prev()
                };
                self.text.replace_range(at..self.caret, "");
                self.caret = at;
                FieldEdit::Changed
            }
            Key::Named(NamedKey::Delete) if self.caret < self.text.len() => {
                let to = self.next();
                self.text.replace_range(self.caret..to, "");
                FieldEdit::Changed
            }
            Key::Named(NamedKey::ArrowLeft) if cmd => {
                self.caret = 0;
                FieldEdit::Moved
            }
            Key::Named(NamedKey::ArrowRight) if cmd => {
                self.end();
                FieldEdit::Moved
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.caret = self.prev();
                FieldEdit::Moved
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.caret = self.next();
                FieldEdit::Moved
            }
            Key::Named(NamedKey::Home) => {
                self.caret = 0;
                FieldEdit::Moved
            }
            Key::Named(NamedKey::End) => {
                self.end();
                FieldEdit::Moved
            }
            Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => FieldEdit::Moved,
            _ => FieldEdit::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use leaf_core::Format;

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.build_visual_unwrapped();
        d
    }

    #[test]
    fn search_ignores_ascii_case_and_does_not_overlap() {
        assert_eq!(search("Row row ROW", "row"), [(0, 3), (4, 7), (8, 11)]);
        assert_eq!(search("aaaa", "aa"), [(0, 2), (2, 4)]);
        assert!(search("anything", "").is_empty());
    }

    #[test]
    fn stepping_wraps_at_either_end() {
        let d = doc("one two one two\n");
        let mut f = Find::default();
        f.query.set("one");
        assert_eq!(f.step(&d, true), Some((0, 3)));
        let mut d = d;
        d.select_range(8, 11);
        assert_eq!(
            f.step(&d, true),
            Some((0, 3)),
            "past the last, round to the first"
        );
        d.select_range(0, 3);
        assert_eq!(
            f.step(&d, false),
            Some((8, 11)),
            "before the first, round to the last"
        );
    }

    #[test]
    fn the_count_names_the_selected_match() {
        let mut d = doc("one two one\n");
        let mut f = Find::default();
        f.query.set("one");
        d.select_range(8, 11);
        assert_eq!(f.count(&d), "2 of 2");
        f.query.set("zebra");
        assert_eq!(f.count(&d), "No results");
    }

    #[test]
    fn replace_all_is_one_undo_step() {
        let mut d = doc("cat and cat and cat\n");
        let mut f = Find::default();
        f.query.set("cat");
        f.replacement.set("dog");
        assert_eq!(replace_all(&mut f, &mut d), 3);
        assert_eq!(d.source, "dog and dog and dog\n");
        d.undo();
        assert_eq!(d.source, "cat and cat and cat\n");
    }

    #[test]
    fn replace_only_replaces_a_selected_match() {
        let mut d = doc("cat dog\n");
        let mut f = Find::default();
        f.query.set("cat");
        f.replacement.set("cow");
        d.select_range(4, 7);
        assert!(!replace_current(&mut f, &mut d), "dog is not a match");
        d.select_range(0, 3);
        assert!(replace_current(&mut f, &mut d));
        assert_eq!(d.source, "cow dog\n");
    }

    #[test]
    fn typing_in_the_query_searches() {
        let mut f = Find {
            open: true,
            focus: Focus::Query,
            ..Find::default()
        };
        assert_eq!(
            f.key(&Key::Character("x".into()), Modifiers::empty()),
            Outcome::Searched
        );
        assert_eq!(f.query.text, "x");
        assert_eq!(
            f.key(&Key::Named(NamedKey::Backspace), Modifiers::empty()),
            Outcome::Searched
        );
        assert_eq!(f.query.text, "");
        assert_eq!(
            f.key(&Key::Named(NamedKey::Enter), Modifiers::SHIFT),
            Outcome::Step { forward: false }
        );
        assert_eq!(
            f.key(&Key::Named(NamedKey::Escape), Modifiers::empty()),
            Outcome::Close
        );
    }

    #[test]
    fn a_paste_into_a_field_asks_for_the_clipboard_and_commits_it() {
        let mut f = Find {
            open: true,
            focus: Focus::Replacement,
            ..Find::default()
        };
        // ⌘V leaves the field as it was: the clipboard is the host's.
        assert_eq!(
            f.key(&Key::Character("v".into()), Modifiers::META),
            Outcome::Paste
        );
        assert_eq!(f.replacement.text, "");
        assert_eq!(f.commit("new"), Outcome::Edited);
        assert_eq!(f.replacement.text, "new");
    }
}
