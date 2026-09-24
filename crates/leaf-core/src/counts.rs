//! Word, character, and paragraph counts over the text a *reader* sees.
//!
//! The numbers a status bar shows are a claim about the document, not about
//! the markup that spells it: `**bold**` is one word and four characters, a
//! link is its label and not its destination, and a picture is a picture. So
//! the tally runs over a [`VisualMap`] — the same rendering the WYSIWYG view
//! paints — rather than over the source string, and cannot drift from what is
//! on the page the way a second, private markdown walk would.
//!
//! What it takes off that map is narrower than what the map draws. A glyph
//! counts only if it is a caret stop ([`Glyph::stop`]) *and* its
//! [`Role`] is one a reader would call text. The first half drops the
//! scaffolding a proportional surface draws but nobody types into — a nested
//! item's indent, a cell's alignment padding — and, because a stop opens a
//! grapheme cluster and never sits inside one, makes "one stop" and "one
//! character" the same statement. The second drops the synthetic furniture:
//! a bullet, a quote's gutter, a rule, a hidden delimiter revealed under
//! [`MarkupMode::Full`](crate::MarkupMode::Full), and the `🖼 alt` / `⧉ name`
//! placeholder a block picture or directive stands in as.
//!
//! [`Glyph::stop`]: crate::wysiwyg::Glyph::stop

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::style::Role;
use crate::wysiwyg::{Glyph, VisualMap};

/// Statistics over the text a reader sees. See [`Doc::counts`] for the rules
/// each field is counted by, and [`crate::counts`] for what "sees" means.
///
/// [`Doc::counts`]: crate::Doc::counts
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextCounts {
    /// Words, by UAX#29 word segmentation: a segment holding at least one
    /// alphabetic or numeric character. So `don't` is one, `3.14` is one, a
    /// lone `—` is none — and `well-known` is **two**, which is what the
    /// algorithm says and what every other UAX#29 counter reports.
    pub words: usize,
    /// Characters as a reader counts them — grapheme clusters, whitespace
    /// included. An emoji family and an accented letter are each one.
    pub characters: usize,
    /// The same, less every whitespace grapheme.
    pub characters_without_spaces: usize,
    /// Block-level text containers holding at least one non-whitespace
    /// character: a paragraph, a heading, each item of a list, each paragraph
    /// inside a blockquote, a whole code block, a whole table.
    pub paragraphs: usize,
}

impl TextCounts {
    /// Fold one block-level container into the tally — its lines, already cut
    /// down to visible text.
    ///
    /// Lines rather than one string because the breaks *between* them are not
    /// characters: a table's cells and a code block's lines each arrive as
    /// their own line, and gluing them with a `\n` would both invent a
    /// character and let the last word of one run into the first of the next.
    /// The block counts as one paragraph however many lines it has, and as
    /// none at all when every line is blank — which is what keeps an empty
    /// paragraph, a rule, and a picture's placeholder out of the count.
    fn add_block(&mut self, lines: &[String]) {
        let mut has_text = false;
        for line in lines {
            for cluster in line.graphemes(true) {
                self.characters += 1;
                if !cluster.chars().all(char::is_whitespace) {
                    self.characters_without_spaces += 1;
                    has_text = true;
                }
            }
            self.words += line.unicode_words().count();
        }
        if has_text {
            self.paragraphs += 1;
        }
    }
}

/// Tally `map`, or the part of it whose source offsets fall inside `range`.
///
/// The map is expected to be an *unwrapped* one — one row per block — which is
/// what makes a row and a paragraph the same thing here. [`Doc::counts`]
/// builds one for the purpose rather than borrowing the frontend's, so that a
/// narrower window cannot mean more paragraphs.
///
/// [`Doc::counts`]: crate::Doc::counts
pub(crate) fn tally(map: &VisualMap, range: Option<Range<usize>>) -> TextCounts {
    let range = range.as_ref();
    let mut counts = TextCounts::default();
    let mut row = 0;
    while row < map.rows.len() {
        // A table is one paragraph, not one per cell — the reader sees a
        // single object, and a two-column shopping list is not twelve
        // paragraphs. Its text comes off the structural grid rather than off
        // the box-drawn picture, which carries borders and column padding the
        // author never wrote.
        if let Some(table) = map.tables.iter().find(|t| t.rows_span.contains(&row)) {
            let cells: Vec<String> = table
                .grid
                .iter()
                .flat_map(|r| r.cells.iter())
                .map(|c| visible(&c.glyphs, range))
                .collect();
            counts.add_block(&cells);
            row = table.rows_span.end.max(row + 1);
            continue;
        }
        // A code block is one paragraph too, however many lines it holds —
        // the same call, for the same reason.
        if let Some(code) = map.code_blocks.iter().find(|c| c.rows_span.contains(&row)) {
            let lines: Vec<String> = map.rows[code.rows_span.clone()]
                .iter()
                .map(|r| visible(&r.glyphs, range))
                .collect();
            counts.add_block(&lines);
            row = code.rows_span.end.max(row + 1);
            continue;
        }
        // The blank gap a block boundary is drawn with is not a block.
        if !map.rows[row].decoration {
            counts.add_block(&[visible(&map.rows[row].glyphs, range)]);
        }
        row += 1;
    }
    counts
}

/// The text of one drawn line: its stop glyphs, less the synthetic ones, less
/// anything whose source byte falls outside `range`.
fn visible(glyphs: &[Glyph], range: Option<&Range<usize>>) -> String {
    glyphs
        .iter()
        .filter(|g| g.stop && is_text(g.style.role))
        .filter(|g| range.is_none_or(|r| r.contains(&g.src)))
        .map(|g| g.ch)
        .collect()
}

/// Whether a glyph in this role is text the reader is reading, as against
/// furniture the renderer drew around it.
///
/// Spelled out arm by arm rather than as a list of exclusions, so that a new
/// [`Role`] has to be answered for here instead of quietly joining whichever
/// side the wildcard fell on.
fn is_text(role: Role) -> bool {
    match role {
        Role::Body | Role::Heading(_) | Role::Code | Role::Link | Role::Mark(_) => true,
        // A bullet, a quote's `│`, a thematic break's dashes and a table's
        // borders: drawn by the renderer, not written by the author.
        Role::ListMarker | Role::ListIndent | Role::QuoteGutter | Role::Rule => false,
        // Raw markup a revealed line is showing — the source, not the text.
        // `Doc::counts` builds its map with no revealed line, so this arm is
        // the statement of intent rather than a live case.
        Role::Delimiter => false,
        // The `🖼 alt` / `⧉ name` stand-in for a block picture, movie, sound,
        // or directive. A reader sees the thing, not the label, and a thing
        // is not a word. (An *inline* image is different: leaf draws its alt
        // text as ordinary prose in the line, and so counts it.)
        Role::Image => false,
        // A formula's atom or placeholder label, for the same reason: what the
        // reader sees is a picture, and `Doc::counts` builds its map as a
        // surface that paints one in a line, so an inline formula is an atom
        // here and never its TeX. (Its TeX on the revealed line is `Code` and
        // would count, but the count reveals nothing.)
        Role::Math => false,
    }
}
