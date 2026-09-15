//! Syntax highlighting for a fenced code block: the lines of a block and the
//! language its fence names in, a [`Token`] per byte range out.
//!
//! The grammars are Sublime Text's, run by syntect, and the set is `two-face`'s
//! — bat's 213, which is the set `plates` publishes with, so a block that
//! highlights in leaf highlights on the published page and the other way
//! round. What comes out is not a colour and not a scope: it is the eight-way
//! [`Token`] classing, which is all a palette can usefully tell apart, decided
//! here once so that four frontends do not each read TextMate scope names.
//!
//! # How a scope becomes a token
//!
//! syntect's parser leaves a stack of scopes over every byte —
//! `source.rust meta.function.rust string.quoted.double.rust
//! punctuation.definition.string.begin.rust` over the `"` that opens a string
//! literal in a function body. The token for a byte is decided the way a
//! stylesheet over classed HTML decides it, because that is what `plates`'
//! stylesheet is and the two should agree:
//!
//! - **Innermost scope first.** The nearest scope that names a token wins, as
//!   an inner `<span>`'s own colour beats what it inherits from an outer one.
//! - **Any atom, not the first.** `punctuation.definition.string.begin` has
//!   both `punctuation` and `string` among its atoms, and the *later* of the
//!   two in [`Token::ALL`] wins — so the quote reads as string, and a `//`
//!   reads as comment.
//! - **`meta` and `source` name nothing.** `meta.function` wraps a whole
//!   function, signature and body together; colouring it would flood
//!   everything nested inside. They are simply absent from the vocabulary.
//!
//! # What it costs
//!
//! The syntax set is unpacked from its embedded dump on first use — tens of
//! milliseconds and about a megabyte of memory — and then shared for the life
//! of the process. Parsing itself is per block, once per rebuild of that block:
//! the WYSIWYG map's block cache keeps a block's rows across edits elsewhere in
//! the document, so typing in a paragraph never re-highlights the code above
//! it, and typing in a code block re-highlights that block alone.

use std::ops::Range;
use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use crate::style::Token;

/// One line's highlighting: byte ranges *into that line*, ascending and
/// non-overlapping, with the token over each. A byte no range covers carries
/// no token.
pub type LineTokens = Vec<(Range<usize>, Token)>;

/// The grammars, unpacked once. `extra_newlines` rather than `extra_no_newlines`
/// because these grammars match a line *with* its terminator — a `//` comment's
/// pattern runs to `\n` — so [`highlight`] feeds each line one, and a set built
/// for the other convention would match differently at every end of line.
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// The atom numbers that name each token, resolved once. A [`Scope`] is a
/// packed list of interned atoms, and interning `keyword` here yields the same
/// number the loaded grammars' `keyword.control.rust` carries in its first
/// slot — scopes are dumped as strings and re-interned on load — so classing
/// a scope is eight `u16` comparisons per atom, no string built.
///
/// Two tokens have two atoms each: `keyword`/`storage` are both
/// [`Token::Keyword`] and `entity`/`variable` both [`Token::Entity`], as in
/// `plates`' stylesheet.
fn atoms() -> &'static [(u16, Token)] {
    static ATOMS: OnceLock<Vec<(u16, Token)>> = OnceLock::new();
    ATOMS.get_or_init(|| {
        [
            ("punctuation", Token::Punctuation),
            ("keyword", Token::Keyword),
            ("storage", Token::Keyword),
            ("entity", Token::Entity),
            ("variable", Token::Entity),
            ("support", Token::Support),
            ("constant", Token::Constant),
            ("string", Token::String),
            ("comment", Token::Comment),
            ("invalid", Token::Invalid),
        ]
        .into_iter()
        // A single-atom scope name always parses; `expect` documents that.
        .map(|(name, t)| {
            (
                Scope::new(name).expect("a bare atom is a scope").atom_at(0),
                t,
            )
        })
        .collect()
    })
}

/// The token one scope names, by the highest-precedence token any of its atoms
/// names — or `None` for a scope like `meta.function` or `source.rust` that
/// names no token at all.
fn token_of_scope(scope: Scope) -> Option<Token> {
    let atoms = atoms();
    (0..scope.len() as usize)
        .map(|i| scope.atom_at(i))
        .filter_map(|a| atoms.iter().find(|(n, _)| *n == a).map(|(_, t)| *t))
        .max_by_key(|t| t.index())
}

/// The token a scope stack puts on the bytes under it: the innermost scope
/// that names one.
fn token_of_stack(stack: &ScopeStack) -> Option<Token> {
    stack
        .as_slice()
        .iter()
        .rev()
        .find_map(|s| token_of_scope(*s))
}

/// Whether `lang` — a fence's info string, trimmed — names a grammar: `rust`,
/// `rs`, `zig` and `swift` do; `text`, `""` and `no-such-language` do not.
pub fn knows(lang: &str) -> bool {
    syntax_set().find_syntax_by_token(lang).is_some()
}

/// Highlight the lines of one fenced code block written in `lang`.
///
/// One `Vec` per line, in order, each a list of byte ranges *into that line*
/// with the token over them, ascending and non-overlapping. A byte no range
/// covers carries no token — an identifier the grammar leaves as plain
/// `source.rust`, the space between two words — and draws in the code colour.
/// The lines are the block's text split on `\n`, without terminators; the
/// terminators are supplied here, since the grammars expect them.
///
/// `None` when `lang` names no grammar the set carries, which is the whole of
/// the difference between "a language we can't highlight" and "a block with
/// nothing worth colouring": a caller draws either in plain code colour, but
/// only the second is a highlighted block.
pub fn highlight(lang: &str, lines: &[&str]) -> Option<Vec<LineTokens>> {
    let set = syntax_set();
    let syntax = set.find_syntax_by_token(lang)?;
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::with_capacity(lines.len());
    let mut buf = String::new();
    for line in lines {
        buf.clear();
        buf.push_str(line);
        buf.push('\n');
        let mut spans: LineTokens = Vec::new();
        // A grammar that fails mid-block — a stack that overflows, a pattern the
        // fancy-regex engine will not run — loses colour from that line on
        // rather than losing the block: the lines so far keep their tokens and
        // the rest carry none, which is what an unknown language would draw.
        let Ok(ops) = state.parse_line(&buf, set) else {
            out.push(spans);
            break;
        };
        let mut last = 0usize;
        // Each op sits at the byte it takes effect from; the bytes since the
        // previous op were under the stack as it stood. The terminator this
        // function added is clipped off — the caller's line has no byte there.
        for (pos, op) in &ops {
            let pos = (*pos).min(line.len());
            if pos > last {
                push_span(&mut spans, last..pos, token_of_stack(&stack));
                last = pos;
            }
            // `apply` fails only on a `Pop` past the stack's bottom, which a
            // grammar does not produce; and the tokens for a line that somehow
            // did would merely be off for the rest of the block.
            let _ = stack.apply(op);
        }
        if line.len() > last {
            push_span(&mut spans, last..line.len(), token_of_stack(&stack));
        }
        out.push(spans);
    }
    // A parse that gave up mid-block leaves the later lines unlisted; pad so a
    // caller can index by line number.
    out.resize_with(lines.len(), Vec::new);
    Some(out)
}

/// Append a range with a token, merging into the previous range when it carries
/// the same token — a run of ops inside one string literal would otherwise
/// leave the literal as a dozen adjacent `String` spans, and a frontend that
/// coalesces glyphs by style merges them anyway. A range with no token is not
/// recorded at all.
fn push_span(spans: &mut LineTokens, range: Range<usize>, token: Option<Token>) {
    let Some(token) = token else { return };
    if let Some((last, t)) = spans.last_mut()
        && *t == token
        && last.end == range.start
    {
        last.end = range.end;
        return;
    }
    spans.push((range, token));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token over byte `at` of line `line`, or `None`.
    fn at(spans: &[LineTokens], line: usize, at: usize) -> Option<Token> {
        spans[line]
            .iter()
            .find(|(r, _)| r.contains(&at))
            .map(|(_, t)| *t)
    }

    #[test]
    fn a_rust_block_is_classed_the_way_a_reader_expects() {
        let lines = [
            "fn main() {",
            "    let s = \"hi\"; // greet",
            "    println!(\"{}\", 42);",
            "}",
        ];
        let spans = highlight("rust", &lines).expect("rust is a known language");
        assert_eq!(spans.len(), lines.len());
        // `fn` and `let` are keywords; `main` at its definition is an entity.
        assert_eq!(at(&spans, 0, 0), Some(Token::Keyword));
        assert_eq!(at(&spans, 0, 3), Some(Token::Entity));
        assert_eq!(at(&spans, 1, 4), Some(Token::Keyword));
        // The string, quotes included — the opening quote's scope is
        // `punctuation.definition.string.begin`, and string outranks punctuation.
        let quote = lines[1].find('"').unwrap();
        assert_eq!(at(&spans, 1, quote), Some(Token::String));
        assert_eq!(at(&spans, 1, quote + 1), Some(Token::String));
        // The comment, its `//` included.
        let slash = lines[1].find("//").unwrap();
        assert_eq!(at(&spans, 1, slash), Some(Token::Comment));
        assert_eq!(at(&spans, 1, slash + 4), Some(Token::Comment));
        // A number is a constant.
        let num = lines[2].find("42").unwrap();
        assert_eq!(at(&spans, 2, num), Some(Token::Constant));
        // Braces are punctuation.
        assert_eq!(at(&spans, 3, 0), Some(Token::Punctuation));
    }

    #[test]
    fn spans_are_ascending_and_within_their_line() {
        let lines = [
            "const x: [u8; 3] = [1, 2, 3]; // n",
            "",
            "fn f() -> u8 { x[0] }",
        ];
        let spans = highlight("rust", &lines).unwrap();
        for (i, line) in spans.iter().enumerate() {
            let mut end = 0;
            for (r, _) in line {
                assert!(r.start >= end, "line {i}: {r:?} overlaps or goes backwards");
                assert!(
                    r.end <= lines[i].len(),
                    "line {i}: {r:?} runs past the line"
                );
                assert!(r.start < r.end, "line {i}: {r:?} is empty");
                end = r.end;
            }
        }
        assert!(spans[1].is_empty(), "an empty line has nothing to class");
    }

    /// A block's grammar state runs across its lines: a block comment opened on
    /// one line is still a comment on the next.
    #[test]
    fn state_carries_from_line_to_line() {
        let lines = ["/* a", "   b */ let c = 1;"];
        let spans = highlight("rust", &lines).unwrap();
        assert_eq!(at(&spans, 1, 3), Some(Token::Comment));
        assert_eq!(at(&spans, 1, 8), Some(Token::Keyword));
    }

    /// The languages this organisation is written in are all in the set —
    /// the reason it is two-face's and not syntect's own.
    #[test]
    fn the_org_languages_are_known() {
        for lang in [
            "rust",
            "rs",
            "zig",
            "swift",
            "toml",
            "typescript",
            "ts",
            "js",
            "sh",
            "md",
        ] {
            assert!(knows(lang), "{lang} should resolve to a grammar");
        }
        assert!(!knows(""));
        assert!(!knows("no-such-language"));
        assert!(highlight("no-such-language", &["x"]).is_none());
    }

    /// Alike tokens over adjacent bytes merge, so a string is one span even
    /// though the grammar pushes and pops several scopes across it.
    #[test]
    fn adjacent_alike_spans_merge() {
        let spans = highlight("rust", &["let s = \"a b c\";"]).unwrap();
        let strings: Vec<_> = spans[0]
            .iter()
            .filter(|(_, t)| *t == Token::String)
            .collect();
        assert_eq!(strings.len(), 1, "{:?}", spans[0]);
        assert_eq!(strings[0].0, 8..15);
    }
}
