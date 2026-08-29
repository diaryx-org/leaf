//! The command palette — one door to every verb in [`crate::commands`].
//!
//! # Why a palette
//!
//! The ⌥-letter namespace was full. Bold, italic, code, highlight, strike,
//! underline, six heading levels, two lists, quote, the task pair, link,
//! language, view, save-as, new, plain paste — that is most of the alphabet
//! already, and the commands still missing (fourteen table operations, three
//! markup modes, two line-flow modes, three media kinds) outnumber the letters
//! left. Inventing chords for the remainder would have produced a keymap nobody
//! could hold and several bindings terminals disagree about anyway.
//!
//! So the rare commands get no key, and the palette gets one: type a few letters
//! of a command's name and press Return. That is also the first thing this
//! editor has ever had that answers "what can I do here?" — which is why
//! commands that *are* unavailable stay in the list, dimmed, with the reason
//! legible from the fact that the whole family is out (no footnotes in an HTML
//! document) rather than silently absent.
//!
//! # Matching
//!
//! A subsequence match, case-insensitive, scored so that what you meant floats
//! up: letters landing at the start of a word score highest, consecutive runs
//! next, anything else counts but barely. `ir` finds "Insert Row Above" ahead of
//! "Horizontal Rule"; `hr` finds the rule. Ties break on the command's position
//! in [`crate::commands::GROUPS`], which is the order a reader would scan.

use ratatui::layout::Rect;

use crate::commands::{Command, Ctx, GROUPS};

/// A palette row: the command, whether this document can run it, and the score
/// that sorted it here (kept only so the sort is stable and inspectable).
#[derive(Clone, Copy)]
pub struct Row {
    pub command: Command,
    pub enabled: bool,
}

/// The palette's state while it's open. Owned by `App` beside the context menu
/// and the text prompt, and like them it takes the keyboard over completely
/// while it's up.
pub struct Palette {
    /// What's been typed. Filters as it grows; empty shows everything.
    pub query: String,
    /// Byte offset into `query`, only ever moved by whole `char`s.
    pub cursor: usize,
    /// Index into `rows` — always on an enabled row when one exists, because a
    /// highlight parked on something that can't run is a Return that does
    /// nothing.
    pub selected: usize,
    /// The filtered, sorted rows. Recomputed whenever `query` changes.
    pub rows: Vec<Row>,
    /// The rect the list was last painted at, stashed for hit-testing exactly as
    /// `MenuLevel::rect` is.
    pub list_rect: Option<Rect>,
    /// How many rows the painted list was scrolled past — the palette can be
    /// longer than the box, so the row under a click is `scrolled_by` further
    /// down `rows` than its screen offset alone would say.
    pub scrolled_by: usize,
}

impl Palette {
    /// Open the palette on an empty query, showing every command with this
    /// document's availability already resolved.
    pub fn new(ctx: &Ctx) -> Self {
        let mut p = Palette {
            query: String::new(),
            cursor: 0,
            selected: 0,
            rows: Vec::new(),
            list_rect: None,
            scrolled_by: 0,
        };
        p.refilter(ctx);
        p
    }

    /// Rebuild `rows` for the current query. Enabled matches first (best score
    /// first), then the unavailable ones — which stay in the list rather than
    /// being filtered out, so a search for "footnote" in an HTML document
    /// answers "there is one, and not here" instead of "no such thing".
    pub fn refilter(&mut self, ctx: &Ctx) {
        let q = self.query.to_lowercase();
        let mut scored: Vec<(i32, usize, Row)> = Vec::new();
        let mut order = 0usize;
        for (group, commands) in GROUPS {
            for command in *commands {
                order += 1;
                // The group name is matched too, so "table" finds every table
                // operation even though none of their labels contains the word.
                let haystack = format!(
                    "{} {}",
                    group.to_lowercase(),
                    command.label().to_lowercase()
                );
                let Some(score) = score(&haystack, &q) else {
                    continue;
                };
                scored.push((
                    score,
                    order,
                    Row {
                        command: *command,
                        enabled: command.enabled(ctx),
                    },
                ));
            }
        }
        // Available first, then by score, then by the order a reader would scan.
        scored.sort_by_key(|(score, order, row)| (!row.enabled, -score, *order));
        self.rows = scored.into_iter().map(|(_, _, row)| row).collect();
        self.selected = self.first_enabled().unwrap_or(0);
    }

    /// The first row that can actually run, if any.
    fn first_enabled(&self) -> Option<usize> {
        self.rows.iter().position(|r| r.enabled)
    }

    /// Move the highlight `delta` rows, skipping unavailable ones and wrapping
    /// at the ends — the same walk `MenuLevel::step` does past section headers,
    /// and for the same reason.
    pub fn step(&mut self, delta: isize) {
        let n = self.rows.len() as isize;
        if n == 0 {
            return;
        }
        let mut i = self.selected as isize;
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if self.rows[i as usize].enabled {
                self.selected = i as usize;
                return;
            }
        }
    }

    /// The row under a screen cell, if the last painted list covers it — the
    /// palette's counterpart to `ContextMenu::hit`, and scrolled by the same
    /// amount the paint was.
    pub fn hit(&self, row: u16, col: u16) -> Option<&Row> {
        let r = self.list_rect?;
        if row < r.y || row >= r.y + r.height || col < r.x || col >= r.x + r.width {
            return None;
        }
        self.rows.get(self.scrolled_by + (row - r.y) as usize)
    }

    /// Whether a screen cell is inside the painted list at all — the difference
    /// between clicking a dimmed row (which holds the palette open) and clicking
    /// the document behind it (which dismisses).
    pub fn covers(&self, row: u16, col: u16) -> bool {
        self.list_rect.is_some_and(|r| {
            row >= r.y && row < r.y + r.height && col >= r.x && col < r.x + r.width
        })
    }

    /// The command Return would run — `None` when nothing in the list can run,
    /// which is what a query matching only unavailable commands leaves behind.
    pub fn chosen(&self) -> Option<Command> {
        self.rows
            .get(self.selected)
            .filter(|r| r.enabled)
            .map(|r| r.command)
    }
}

/// Score `query` as a subsequence of `haystack`, or `None` if it isn't one. An
/// empty query matches everything at zero, which is what shows the whole list
/// the moment the palette opens.
///
/// The weights are the whole of the ranking: a letter that starts a word is
/// worth far more than one in the middle, and a letter continuing a run is worth
/// more than a lone one. That is enough to put "Insert Row Above" above
/// "Horizontal Rule" for `ir` without anything resembling a real fuzzy matcher.
fn score(haystack: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().collect();
    // Two passes. The first prefers a word-start occurrence of each letter over
    // a nearer one buried mid-word, which is what makes `ir` read as the
    // initials of "Insert Row" rather than as the `i` and `r` inside "insert".
    // Being greedy, that preference can walk past the only match there was
    // ("xa b" for `ab` reaches the `a` in a later word and finds no `b` after
    // it), so a failure falls back to the plain nearest-first walk. Both are one
    // scan; a real fuzzy matcher's DP buys nothing on labels this short.
    walk(&hay, query, true).or_else(|| walk(&hay, query, false))
}

/// One greedy pass over `hay`, matching each of `query`'s letters in turn.
/// With `prefer_word_start`, each letter takes the next occurrence that begins a
/// word when there is one, and the plain next occurrence otherwise.
fn walk(hay: &[char], query: &str, prefer_word_start: bool) -> Option<i32> {
    let word_start = |i: usize| i == 0 || !hay[i - 1].is_alphanumeric();
    let mut total = 0;
    let mut at = 0usize;
    let mut previous_match: Option<usize> = None;
    for want in query.chars() {
        if want == ' ' {
            continue; // a space in the query is a separator, not a letter to find
        }
        let nearest = hay[at..].iter().position(|c| *c == want).map(|i| i + at);
        let found = match prefer_word_start {
            true => hay[at..]
                .iter()
                .enumerate()
                .find(|(i, c)| **c == want && word_start(i + at))
                .map(|(i, _)| i + at)
                .or(nearest)?,
            false => nearest?,
        };
        // Word starts are what people actually type the initials of.
        total += if word_start(found) { 8 } else { 1 };
        if previous_match == Some(found.wrapping_sub(1)) {
            total += 4;
        }
        previous_match = Some(found);
        at = found + 1;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use leaf_core::Doc;

    fn ctx(source: &str, format: leaf_core::Format) -> Ctx {
        let mut doc = Doc::from_source(source.into(), format).unwrap();
        Ctx::read(&mut doc)
    }

    #[test]
    fn an_empty_query_lists_everything() {
        let p = Palette::new(&ctx("hi\n", leaf_core::Format::Markdown));
        let total: usize = GROUPS.iter().map(|(_, c)| c.len()).sum();
        assert_eq!(p.rows.len(), total);
    }

    #[test]
    fn a_non_subsequence_matches_nothing() {
        assert_eq!(score("horizontal rule", "zzz"), None);
    }

    #[test]
    fn word_initials_outrank_letters_buried_mid_word() {
        // Both match "ir": the first on two word initials, the second on an `i`
        // buried in "horizontal" and an `r` that happens to start "rule".
        let initials = score("insert row above", "ir").unwrap();
        let buried = score("horizontal rule", "ir").unwrap();
        assert!(
            initials > buried,
            "initials {initials} should outrank buried {buried}"
        );
    }

    #[test]
    fn typing_filters_and_the_highlight_lands_on_a_runnable_row() {
        let ctx = ctx("hi\n", leaf_core::Format::Markdown);
        let mut p = Palette::new(&ctx);
        p.query = "footnote".into();
        p.refilter(&ctx);
        assert!(!p.rows.is_empty());
        assert_eq!(p.chosen(), Some(Command::Footnote));
    }

    /// The reason unavailable commands stay in the list: searching for one in a
    /// format that can't spell it should say so, not come back empty.
    #[test]
    fn an_unavailable_command_is_listed_but_not_runnable() {
        let ctx = ctx("<p>hi</p>\n", leaf_core::Format::Html);
        let mut p = Palette::new(&ctx);
        p.query = "footnote".into();
        p.refilter(&ctx);
        assert!(
            p.rows
                .iter()
                .any(|r| r.command == Command::Footnote && !r.enabled),
            "the footnote command should be listed and dimmed"
        );
        assert_ne!(p.chosen(), Some(Command::Footnote));
    }

    /// The group name is part of the haystack, so the word for a family finds
    /// every member even though no label contains it.
    #[test]
    fn a_group_name_finds_its_whole_family() {
        let ctx = ctx("| a |\n| - |\n| b |\n", leaf_core::Format::Markdown);
        let mut p = Palette::new(&ctx);
        p.query = "table".into();
        p.refilter(&ctx);
        assert!(p.rows.iter().filter(|r| r.enabled).count() >= 10);
    }

    #[test]
    fn stepping_skips_unavailable_rows() {
        let ctx = ctx("<p>hi</p>\n", leaf_core::Format::Html);
        let mut p = Palette::new(&ctx);
        for _ in 0..p.rows.len() + 2 {
            p.step(1);
            assert!(p.rows[p.selected].enabled);
        }
    }
}
