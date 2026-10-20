//! The system's spelling checker and automatic substitutions, for the
//! editor: misspellings underlined as the text settles, guesses in the
//! context menu, Check Document Now, and what `NSTextView` does to text as it
//! is typed — a misspelling corrected, a text replacement expanded, a quote
//! curled, a `--` made a dash. The SwiftUI app's `Spelling.swift` is where
//! each rule was worked out (LeafUI's `LeafTextView` before it); this is
//! that, in Rust.
//!
//! Every range is a UTF-16 range of the text the system sees (`text.rs`):
//! the visible text in the rendered view, its markup hidden. The checker
//! reads the same text with all but prose blanked, so code and bullets are
//! never flagged and an index into one is an index into the other. Nothing
//! is checked or substituted in the source view.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::NonNull;
use std::time::Duration;

use dioxus_native::winit::window::WindowId;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{
    NSArray, NSInteger, NSOrthography, NSRange, NSString, NSTextCheckingResult, NSTextCheckingType,
    NSUserDefaults,
};

use crate::app;
use crate::commands::{Command, Switch};
use crate::page::{Chrome, Page};
use crate::text;

/// The switches under Spelling and Grammar and Substitutions, for every
/// editor in the app. The substitutions start as the system's settings
/// (Keyboard ▸ Text Input).
struct Switches {
    while_typing: Cell<bool>,
    correct: Cell<bool>,
    quotes: Cell<bool>,
    dashes: Cell<bool>,
    replacement: Cell<bool>,
}

const WHILE_TYPING: &str = "CheckSpellingWhileTyping";

thread_local! {
    static SWITCHES: Switches = const {
        Switches {
            while_typing: Cell::new(true),
            correct: Cell::new(true),
            quotes: Cell::new(true),
            dashes: Cell::new(true),
            replacement: Cell::new(true),
        }
    };
    static PAGES: RefCell<HashMap<WindowId, PageSpelling>> = RefCell::default();
}

/// One editor's spelling: its document in the checker, and what it has
/// found.
struct PageSpelling {
    /// What Ignore Spelling remembers a word for.
    tag: NSInteger,
    /// The text last scheduled for a check: its revision, and whether it
    /// was the source view's.
    scheduled: Option<(u64, bool)>,
    /// The misspellings found in the text at a revision.
    found: (u64, Vec<(usize, usize)>),
    /// Advanced by every schedule, so an older answer is dropped.
    generation: u64,
}

impl PageSpelling {
    fn new() -> Self {
        Self {
            tag: NSSpellChecker::uniqueSpellDocumentTag(),
            scheduled: None,
            found: (0, Vec::new()),
            generation: 0,
        }
    }
}

fn with_page<R>(id: WindowId, f: impl FnOnce(&mut PageSpelling) -> R) -> R {
    PAGES.with_borrow_mut(|p| f(p.entry(id).or_insert_with(PageSpelling::new)))
}

fn checker() -> objc2::rc::Retained<NSSpellChecker> {
    NSSpellChecker::sharedSpellChecker()
}

/// The switches as the system and the last launch left them.
pub fn start() {
    let d = NSUserDefaults::standardUserDefaults();
    let typing = d
        .objectForKey(&NSString::from_str(WHILE_TYPING))
        .map(|_| d.boolForKey(&NSString::from_str(WHILE_TYPING)))
        .unwrap_or(true);
    SWITCHES.with(|s| {
        s.while_typing.set(typing);
        s.correct
            .set(NSSpellChecker::isAutomaticSpellingCorrectionEnabled());
        s.quotes
            .set(NSSpellChecker::isAutomaticQuoteSubstitutionEnabled());
        s.dashes
            .set(NSSpellChecker::isAutomaticDashSubstitutionEnabled());
        s.replacement
            .set(NSSpellChecker::isAutomaticTextReplacementEnabled());
    });
}

fn cell(s: &Switches, which: Switch) -> &Cell<bool> {
    match which {
        Switch::WhileTyping => &s.while_typing,
        Switch::Correct => &s.correct,
        Switch::Quotes => &s.quotes,
        Switch::Dashes => &s.dashes,
        Switch::Replacement => &s.replacement,
    }
}

pub fn is_on(which: Switch) -> bool {
    SWITCHES.with(|s| cell(s, which).get())
}

/// The switches that are on.
pub fn switches() -> Vec<Switch> {
    [
        Switch::WhileTyping,
        Switch::Correct,
        Switch::Quotes,
        Switch::Dashes,
        Switch::Replacement,
    ]
    .into_iter()
    .filter(|s| is_on(*s))
    .collect()
}

/// A menu's switch: flipped, and every page checked again or cleared.
pub fn toggle(which: Switch) {
    let now = SWITCHES.with(|s| {
        let c = cell(s, which);
        c.set(!c.get());
        c.get()
    });
    if which == Switch::WhileTyping {
        NSUserDefaults::standardUserDefaults()
            .setBool_forKey(now, &NSString::from_str(WHILE_TYPING));
        for p in app::pages() {
            schedule(&p, true);
        }
    }
}

/// Edit ▸ Substitutions ▸ Show Substitutions.
pub fn show_substitutions() {
    checker().substitutionsPanel(super::mtm()).orderFront(None);
}

fn checks(chrome: &Chrome) -> bool {
    is_on(Switch::WhileTyping) && !chrome.source_view
}

/// The page's chrome was read again: a change of text, or of view, is
/// checked once the typing pauses; a move of the caret is not.
pub fn changed(page: &Page, chrome: &Chrome) {
    let id = page.window.id();
    let now = (chrome.revision, chrome.source_view);
    let fresh = with_page(id, |s| {
        let fresh = s.scheduled != Some(now);
        s.scheduled = Some(now);
        fresh
    });
    if fresh {
        schedule(page, false);
    }
}

/// The page's window has gone.
pub fn closed(id: WindowId) {
    if let Some(s) = PAGES.with_borrow_mut(|p| p.remove(&id)) {
        checker().closeSpellDocumentWithTag(s.tag);
    }
}

fn schedule(page: &Page, force: bool) {
    let id = page.window.id();
    let generation = with_page(id, |s| {
        s.generation += 1;
        s.generation
    });
    let Some(chrome) = page.chrome() else { return };
    if !checks(&chrome) {
        with_page(id, |s| s.found = (chrome.revision, Vec::new()));
        page.with_editor(|ed, _| ed.set_misspellings(chrome.revision, Vec::new()));
        return;
    }
    let delay = if force { 0 } else { 300 };
    let when = dispatch2::DispatchTime::try_from(Duration::from_millis(delay))
        .unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, move || {
        app::on_turn(move || check(id, generation));
    });
}

/// Ask the checker about the page's prose, and underline what it answers
/// if nothing has been scheduled since.
fn check(id: WindowId, generation: u64) {
    let Some(page) = app::page(id) else { return };
    if with_page(id, |s| s.generation) != generation {
        return;
    }
    let Some((revision, prose)) = page.with_doc(|d| (d.revision(), text::prose(d))) else {
        return;
    };
    let units: Vec<u16> = prose.encode_utf16().collect();
    if units.is_empty() {
        return;
    }
    let tag = with_page(id, |s| s.tag);
    let whole = NSRange::new(0, units.len());
    let block = block2::RcBlock::new(
        move |_seq: NSInteger,
              results: NonNull<NSArray<NSTextCheckingResult>>,
              _orthography: NonNull<NSOrthography>,
              _words: NSInteger| {
            // On the checker's own thread: read the ranges here, and hand
            // them to the main thread as plain numbers.
            let results = unsafe { results.as_ref() };
            let ranges: Vec<(usize, usize)> = results
                .iter()
                .filter(|r| r.resultType().contains(NSTextCheckingType::Spelling))
                .map(|r| r.range())
                .filter(|r| !hyphenated(&units, r.location, r.length))
                .map(|r| (r.location, r.length))
                .collect();
            dispatch2::DispatchQueue::main().exec_async(move || {
                app::on_turn(move || found(id, generation, revision, ranges));
            });
        },
    );
    unsafe {
        checker()
            .requestCheckingOfString_range_types_options_inSpellDocumentWithTag_completionHandler(
                &NSString::from_str(&prose),
                whole,
                NSTextCheckingType::Spelling.0,
                None,
                tag,
                Some(&block),
            );
    }
}

/// The checker answered: underline what it found, if the page is where it
/// was when it asked.
fn found(id: WindowId, generation: u64, revision: u64, ranges: Vec<(usize, usize)>) {
    let Some(page) = app::page(id) else { return };
    if with_page(id, |s| s.generation) != generation {
        return;
    }
    with_page(id, |s| s.found = (revision, ranges.clone()));
    page.with_editor(|ed, _| {
        let source = {
            let mut d = ed.doc();
            if d.revision() != revision {
                return;
            }
            ranges
                .iter()
                .map(|&(loc, len)| text::bounds(&mut d, loc, len))
                .filter(|(from, to)| from < to)
                .collect()
        };
        ed.set_misspellings(revision, source);
    });
}

/// Apple's checker flags some ordinary editorial compounds
/// ("round-trippable"). A hyphen joins the token deliberately, so neither
/// the compound nor a part of it is underlined.
fn hyphenated(units: &[u16], loc: usize, len: usize) -> bool {
    const HYPHENS: &[char] = &['-', '‐', '‑', '‒', '–', '—'];
    let ch = |i: usize| char::from_u32(units[i] as u32).unwrap_or('a');
    let token = |c: char| c.is_alphabetic() || HYPHENS.contains(&c) || c == '\'' || c == '’';
    if loc + len > units.len() {
        return false;
    }
    let (mut start, mut end) = (loc, loc + len);
    while start > 0 && token(ch(start - 1)) {
        start -= 1;
    }
    while end < units.len() && token(ch(end)) {
        end += 1;
    }
    (start..end).any(|i| HYPHENS.contains(&ch(i)))
}

/// Whether typing `typed` ends the word before it: a space or punctuation,
/// not an apostrophe or a hyphen, which a word goes on through.
fn ends_word(typed: &str) -> bool {
    typed.chars().next().is_some_and(|c| {
        (c.is_whitespace() || c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_control()))
            && !"'’-‐".contains(c)
    })
}

/// The misspelled word the caret stands in or at the end of, as (UTF-16
/// location, length, the word). If the check since the last edit has not
/// answered yet, the checker is asked for the caret's paragraph there and
/// then, so a word just typed still gets its guesses.
pub fn at_caret(page: &Page) -> Option<(usize, usize, String)> {
    let id = page.window.id();
    let (revision, prose, visible, caret, source) = page.with_doc(|d| {
        let (a, b) = d.selection().unwrap_or((d.caret, d.caret));
        let caret = (a == b).then(|| text::utf16_of(d, a));
        (
            d.revision(),
            text::prose(d),
            text::visible(d),
            caret,
            d.view == leaf_core::View::Source,
        )
    })?;
    let at = caret?;
    if source {
        return None;
    }
    let vis: Vec<u16> = visible.encode_utf16().collect();
    let word =
        |loc: usize, len: usize| String::from_utf16_lossy(&vis[loc..(loc + len).min(vis.len())]);
    let hit = |&(loc, len): &(usize, usize)| at >= loc && at <= loc + len;
    let (rev, found) = with_page(id, |s| s.found.clone());
    if rev == revision {
        return found
            .iter()
            .find(|r| hit(r))
            .map(|&(l, n)| (l, n, word(l, n)));
    }
    let units: Vec<u16> = prose.encode_utf16().collect();
    let ns = NSString::from_str(&prose);
    let tag = with_page(id, |s| s.tag);
    let mut start = paragraph(&units, at.min(units.len())).0;
    while start < units.len() {
        let r = unsafe {
            checker()
                .checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                    &ns,
                    start as NSInteger,
                    None,
                    false,
                    tag,
                    std::ptr::null_mut(),
                )
        };
        if r.location == objc2_foundation::NSNotFound as usize || r.location > at {
            return None;
        }
        if hit(&(r.location, r.length)) && !hyphenated(&units, r.location, r.length) {
            return Some((r.location, r.length, word(r.location, r.length)));
        }
        start = (start + 1).max(r.location + r.length);
    }
    None
}

/// The context menu's spelling items for the misspelled word at the
/// caret: the checker's guesses, then Ignore and Learn.
pub fn context_items(page: &Page) -> Vec<(String, Option<Command>)> {
    let Some((loc, len, word)) = at_caret(page) else {
        return Vec::new();
    };
    let Some(visible) = page.with_doc(text::visible) else {
        return Vec::new();
    };
    let tag = with_page(page.window.id(), |s| s.tag);
    let guesses = checker()
        .guessesForWordRange_inString_language_inSpellDocumentWithTag(
            NSRange::new(loc, len),
            &NSString::from_str(&visible),
            None,
            tag,
        )
        .map(|g| g.iter().map(|s| s.to_string()).take(8).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut items: Vec<(String, Option<Command>)> = if guesses.is_empty() {
        vec![("No Guesses Found".into(), None)]
    } else {
        guesses
            .into_iter()
            .map(|g| (g.clone(), Some(Command::Replace(loc, len, word.clone(), g))))
            .collect()
    };
    items.push((String::new(), None));
    items.push((
        "Ignore Spelling".into(),
        Some(Command::IgnoreSpelling(word.clone())),
    ));
    items.push(("Learn Spelling".into(), Some(Command::LearnSpelling(word))));
    items
}

/// A guess chosen: the word it was for, if it still stands there, replaced.
pub fn replace(page: &Page, loc: usize, len: usize, word: &str, with: &str) {
    page.with_editor(|ed, _| {
        let done = {
            let mut d = ed.doc();
            match text::exact(&mut d, loc, len, word) {
                Some((from, to)) => {
                    d.edit(from, to, with);
                    true
                }
                None => false,
            }
        };
        if done {
            ed.refresh();
        }
    });
}

pub fn ignore(page: &Page, word: &str) {
    let tag = with_page(page.window.id(), |s| s.tag);
    checker().ignoreWord_inSpellDocumentWithTag(&NSString::from_str(word), tag);
    schedule(page, true);
}

pub fn learn(page: &Page, word: &str) {
    checker().learnWord(&NSString::from_str(word));
    schedule(page, true);
}

/// Edit ▸ Spelling and Grammar ▸ Check Document Now: select the next
/// misspelled word after the selection, from the top once past the end.
pub fn check_document(page: &Page) {
    let Some((prose, after)) = page.with_doc(|d| {
        let (_, b) = d.selection().unwrap_or((d.caret, d.caret));
        (text::prose(d), text::utf16_of(d, b))
    }) else {
        return;
    };
    let units: Vec<u16> = prose.encode_utf16().collect();
    let ns = NSString::from_str(&prose);
    let tag = with_page(page.window.id(), |s| s.tag);
    let mut start = after;
    for _ in 0..2 {
        let r = unsafe {
            checker()
                .checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                    &ns,
                    start as NSInteger,
                    None,
                    true,
                    tag,
                    std::ptr::null_mut(),
                )
        };
        if r.location == objc2_foundation::NSNotFound as usize {
            break;
        }
        if !hyphenated(&units, r.location, r.length) {
            page.with_editor(|ed, _| {
                {
                    let mut d = ed.doc();
                    let (from, to) = text::bounds(&mut d, r.location, r.length);
                    d.select_range(from, to);
                }
                ed.refresh();
            });
            return;
        }
        start = r.location + r.length;
    }
    objc2_app_kit::NSBeep();
}

/// One substitution the checker proposed for what was just typed.
#[derive(Debug, PartialEq)]
struct Substitution {
    loc: usize,
    len: usize,
    with: String,
}

/// Make the substitutions the keystroke `typed` completes, as `NSTextView`
/// makes them, through core's `substitute`: the caret stays where the
/// typing left it, and they are one undo step, apart from the typing, so ⌘Z
/// puts back exactly what was typed.
pub fn after_typing(page: &Page, typed: &str) {
    let (quotes, dashes) = (is_on(Switch::Quotes), is_on(Switch::Dashes));
    let word_end = ends_word(typed) && (is_on(Switch::Replacement) || is_on(Switch::Correct));
    let may_quote = quotes && typed.contains(['"', '\'']);
    let may_dash = dashes && !typed.starts_with('-');
    if !(may_quote || word_end || may_dash) {
        return;
    }
    let Some(Some((prose, visible, caret))) = page.with_doc(|d| {
        if d.view == leaf_core::View::Source || d.selection().is_some() {
            return None;
        }
        Some((text::prose(d), text::visible(d), text::utf16_of(d, d.caret)))
    }) else {
        return;
    };
    let units: Vec<u16> = prose.encode_utf16().collect();
    if may_dash && !may_quote && !word_end {
        // Only worth asking when a `--` stands just behind.
        let typed_len = typed.encode_utf16().count();
        let back = caret.saturating_sub(typed_len + 2);
        let behind =
            String::from_utf16_lossy(&units[back.min(units.len())..caret.min(units.len())]);
        if !behind.contains("--") {
            return;
        }
    }
    let found = substitutions(page, typed, &prose, &units, caret);
    if found.is_empty() {
        return;
    }
    let vis: Vec<u16> = visible.encode_utf16().collect();
    page.with_editor(|ed, _| {
        {
            let mut d = ed.doc();
            d.begin_undo_group();
            // Last first, so each earlier range still stands where it was.
            for s in &found {
                if s.loc + s.len > vis.len() {
                    continue;
                }
                let original = String::from_utf16_lossy(&vis[s.loc..s.loc + s.len]);
                if let Some((from, to)) = text::exact(&mut d, s.loc, s.len, &original) {
                    d.substitute(from, to, &s.with);
                }
            }
            d.end_undo_group();
        }
        ed.refresh();
    });
}

/// What the system would substitute now that `typed` has been typed at
/// UTF-16 `caret`, only in the caret's paragraph and only what the
/// keystroke itself completes:
///
/// - a straight quote is curled as it is typed, by what stands before it;
/// - a `--` becomes a dash once the character after it is typed, and that
///   character is not another hyphen, so `---` is left for the rule it is;
/// - a word is corrected, or a text replacement expanded, as the character
///   that ends it is typed.
///
/// Last first.
fn substitutions(
    page: &Page,
    typed: &str,
    text: &str,
    units: &[u16],
    caret: usize,
) -> Vec<Substitution> {
    let typed_len = typed.encode_utf16().count();
    if typed_len == 0 || caret < typed_len || caret > units.len() {
        return Vec::new();
    }
    let typed_start = caret - typed_len;
    let word_end = ends_word(typed);
    let checker = checker();
    let language = checker.language();
    let correcting = word_end
        && is_on(Switch::Correct)
        && !checker.preventsAutocorrectionBeforeString_language(
            &NSString::from_str(typed),
            Some(&language),
        );
    let mut types = NSTextCheckingType::empty();
    if is_on(Switch::Quotes) {
        types |= NSTextCheckingType::Quote;
    }
    if is_on(Switch::Dashes) {
        types |= NSTextCheckingType::Dash;
    }
    if word_end && is_on(Switch::Replacement) {
        types |= NSTextCheckingType::Replacement;
    }
    if correcting {
        types |= NSTextCheckingType::Spelling | NSTextCheckingType::Correction;
    }
    if types.is_empty() {
        return Vec::new();
    }
    let ns = NSString::from_str(text);
    let (para, _) = paragraph(units, typed_start);
    let checked = NSRange::new(para, caret - para);
    let tag = with_page(page.window.id(), |s| s.tag);
    let results = unsafe {
        checker.checkString_range_types_options_inSpellDocumentWithTag_orthography_wordCount(
            &ns,
            checked,
            types.0,
            None,
            tag,
            None,
            std::ptr::null_mut(),
        )
    };
    let sub = |loc: usize, len: usize| {
        String::from_utf16_lossy(&units[loc..(loc + len).min(units.len())])
    };
    let mut found = Vec::new();
    let mut word_taken = false;
    let results: Vec<_> = results.iter().collect();
    for r in &results {
        let range = r.range();
        let (loc, len) = (range.location, range.length);
        let Some(with) = r
            .replacementString()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let ty = r.resultType();
        let keep = if ty == NSTextCheckingType::Quote {
            loc < caret && loc + len > typed_start
        } else if ty == NSTextCheckingType::Dash {
            // Two hyphens exactly: a third either side is a rule, or
            // someone's ASCII art, and not a dash.
            loc + len == typed_start
                && !typed.starts_with('-')
                && sub(loc, len) == "--"
                && (loc == 0 || sub(loc - 1, 1) != "-")
        } else if ty == NSTextCheckingType::Replacement {
            let ok = loc + len == typed_start;
            word_taken |= ok;
            ok
        } else {
            false
        };
        if keep {
            found.push(Substitution { loc, len, with });
        }
    }
    if correcting && !word_taken {
        // The checker reports a word it would correct as a correction where
        // the system's own setting is on, and as a plain misspelling where
        // it is off; the correction for the latter is asked for by word.
        for r in &results {
            let range = r.range();
            if range.location + range.length != typed_start {
                continue;
            }
            let ty = r.resultType();
            let with = if ty == NSTextCheckingType::Correction {
                r.replacementString().map(|s| s.to_string())
            } else if ty == NSTextCheckingType::Spelling
                && !hyphenated(units, range.location, range.length)
            {
                checker
                    .correctionForWordRange_inString_language_inSpellDocumentWithTag(
                        range, &ns, &language, tag,
                    )
                    .map(|s| s.to_string())
            } else {
                continue;
            };
            if let Some(with) =
                with.filter(|w| !w.is_empty() && *w != sub(range.location, range.length))
            {
                found.push(Substitution {
                    loc: range.location,
                    len: range.length,
                    with,
                });
                break;
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.loc));
    found
}

/// The paragraph around UTF-16 `at`: from after the newline before it to
/// after the one past it.
fn paragraph(units: &[u16], at: usize) -> (usize, usize) {
    let nl = b'\n' as u16;
    let at = at.min(units.len());
    let start = units[..at]
        .iter()
        .rposition(|&u| u == nl)
        .map_or(0, |i| i + 1);
    let end = units[at..]
        .iter()
        .position(|&u| u == nl)
        .map_or(units.len(), |i| at + i + 1);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn a_hyphenated_compound_is_never_flagged() {
        let t = u("a round-trippable value");
        // "trippable" alone, and the whole compound.
        assert!(hyphenated(&t, 8, 9));
        assert!(hyphenated(&t, 2, 15));
        assert!(!hyphenated(&t, 18, 5));
    }

    #[test]
    fn a_word_ends_at_a_space_or_punctuation_but_not_an_apostrophe() {
        assert!(ends_word(" "));
        assert!(ends_word("."));
        assert!(!ends_word("'"));
        assert!(!ends_word("-"));
        assert!(!ends_word("a"));
    }

    #[test]
    fn a_paragraph_is_bounded_by_newlines() {
        let t = u("one\ntwo three\nfour");
        assert_eq!(paragraph(&t, 6), (4, 14));
        assert_eq!(paragraph(&t, 0), (0, 4));
        assert_eq!(paragraph(&t, 16), (14, 18));
    }
}
