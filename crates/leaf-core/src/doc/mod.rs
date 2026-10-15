//! The document model: a `twig::Editor` plus a byte-offset caret and selection.
//!
//! Where bough moves a selection through the *tree*, leaf moves a *caret*
//! through the *characters* — a normal text editor's model — and expresses
//! every mutation as one of twig's offset-addressed ops:
//!
//!   - typing / delete  → `edit_range(start, end, text)`   (P0)
//!   - re-anchoring      → the returned `Change`            (P1)
//!   - cursor context    → `node_at` / `ancestors_at`       (P3)
//!   - the toolbar       → `wrap_range`/`toggle_inline`/`set_block`,
//!                         `toggle_block_container`/`insert_link`   (P5)
//!
//! twig reparses after every edit and leaves everything outside the splice
//! byte-for-byte untouched, so the document stays a live, navigable AST while
//! you type into it.

// `PathBuf` names the `path` field and the untitled marker on every build;
// `Path` is only touched by the filesystem I/O gated behind the `fs` feature.
// The docs in this file lay their `- key → meaning` lists out in aligned
// columns, which puts a continuation line further right than clippy's
// list-indent rule likes. A lazy continuation renders as the same paragraph
// either way, and the alignment is what makes those tables readable, so the
// layout wins over the lint.
#![allow(clippy::doc_overindented_list_items)]

use std::collections::HashMap;
use std::ops::Range;
#[cfg(feature = "fs")]
use std::path::Path;
use std::path::PathBuf;

#[cfg(feature = "fs")]
use anyhow::Context;
use anyhow::{Result, anyhow};
use twig::{
    Alignment, BlockContainerKind, BlockKind, Change, Editor, FlatNode, Format, Gesture,
    InlineKind, Kind, MarkdownExtensions, NodeId, QueryMatch,
};
use unicode_segmentation::GraphemeCursor;

use crate::counts::{self, TextCounts};
use crate::html;
use crate::source::{self, SourceMap};
use crate::style::{Align, FontFace, FontSize, LineHeight, MarkColor, TextColor};
use crate::wysiwyg::{self, MediaKind, MediaStop, Reveal, VisualMap};

mod blocks;
mod caret;
mod clipboard;
mod delete;
mod editing;
mod file;
mod indent;
mod insert;
mod marks;
mod move_block;
mod newline;
mod presentation;
mod splice;
mod state;
mod tables;
mod tasks;
mod text;
mod under_caret;
mod undo;

use text::*;

/// The name of the leaf directive a page break is — [`Doc::insert_page_break`]
/// writes it and the walker draws it, and a frontend that paginates matches a
/// [`DirectiveMark`](crate::wysiwyg::DirectiveMark) against it. One spelling,
/// stated once.
pub const PAGE_BREAK: &str = "page-break";

/// Which view the body shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// The raw document with a caret in source bytes.
    Source,
    /// Markup resolved to real styles, caret riding the rendered glyphs.
    Wysiwyg,
}

/// How much of the source markup the WYSIWYG view exposes — a per-editor
/// preference, orthogonal to [`View`]. Named for markup rather than for Markdown
/// because leaf is grammar-agnostic: twig hands it Djot, HTML and XML on the same
/// terms, and every rung below is about *delimiters*, whatever grammar spells
/// them. The examples are Markdown only because that is what most documents are.
///
/// A single ladder over two underlying axes, because only three of their four
/// combinations are coherent:
///
/// | | authoring off | authoring on |
/// |---|---|---|
/// | delimiters hidden | [`None`](Self::None) | [`Shortcuts`](Self::Shortcuts) |
/// | caret line revealed | *incoherent* | [`Full`](Self::Full) |
///
/// The empty quadrant would show delimiters on the caret's line and then escape
/// the ones you type — a surface that displays a syntax it refuses to accept.
/// Someone who wants to read raw markup without authoring it has
/// [`View::Source`], which is the better tool for it.
///
/// The two axes are read separately by the code that cares — see
/// [`reveals_caret_line`](Self::reveals_caret_line) and
/// [`authors`](Self::authors) — so neither behaviour has to know it's spelled
/// as a ladder.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MarkupMode {
    /// Delimiters stay hidden even on the caret's line, and typed syntax stays
    /// literal — twig escapes anything that would open markup, so formatting
    /// comes from commands (⌘b, the toolbar) instead of from spelling. The clean
    /// reading surface for people who don't write markup by hand; the default,
    /// and what Diaryx ships.
    #[default]
    None,
    /// Delimiters stay hidden, but typing them authors real markup: `*x*`
    /// becomes italic and the asterisks disappear into the styling
    /// (Typora/Bear-shaped). For someone who knows the syntax but wants the
    /// clean surface back once it has been applied.
    Shortcuts,
    /// The caret's line shows its raw markup while every other line renders
    /// resolved (Obsidian live-preview-shaped), and typed syntax authors markup
    /// — for people fluent in the document's grammar who want to see and edit
    /// the delimiters they type.
    Full,
}

impl MarkupMode {
    /// Whether the rich view shows raw delimiters on the line holding the caret.
    /// The rendering axis — read by [`Doc::reveal_line`] and threaded into the
    /// WYSIWYG builder.
    pub fn reveals_caret_line(self) -> bool {
        matches!(self, MarkupMode::Full)
    }

    /// Whether typed markup characters author real formatting. The editing axis
    /// — read by [`Doc::insert`], which escapes typed syntax when this is false.
    pub fn authors(self) -> bool {
        !matches!(self, MarkupMode::None)
    }
}

/// How the WYSIWYG view treats a *soft break* — a bare newline inside a
/// paragraph. An axis of its own, orthogonal to [`MarkupMode`] (which governs
/// inline-markup delimiters) and to [`View`]: any reveal preference pairs with
/// either flow. The renderer consults it when it lays a block's inline content
/// into visual rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LineFlow {
    /// A soft break folds into a space and the paragraph reflows to the
    /// viewport width — flowing prose, where the source's line wrapping is
    /// insignificant. The default, and what Diaryx ships.
    #[default]
    Fold,
    /// A soft break renders as a line break exactly where it was written, so
    /// the author's source line structure shows on screen unchanged — the mode
    /// for people who lay out their prose deliberately (one sentence or clause
    /// per line, semantic line breaks). The break is still a soft break in the
    /// source; only its rendering changes.
    Preserve,
}

/// What the file behind a document looks like right now, against the bytes leaf
/// last read from it or wrote to it — the question a frontend asks before it
/// saves (a `Changed` file plus a `dirty` document is an overwrite about to
/// happen) or when its window regains focus. See [`Doc::disk_state`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskState {
    /// The file holds exactly the bytes leaf last read or wrote.
    Unchanged,
    /// Someone else wrote the file since. Saving overwrites their work; see
    /// [`Doc::reload`] for the other direction.
    Changed,
    /// The file is gone — deleted or renamed away. A save recreates it.
    Missing,
    /// There is a path, but the file couldn't be read (permissions, a directory
    /// in the way): leaf can't tell, and won't guess.
    Unreadable,
    /// No file behind this document yet — see [`Doc::blank`]. Nothing can have
    /// changed under a document that was never on disk.
    Untitled,
}

/// The inline marks in force at a point in the document — what a toolbar
/// lights up. A `Copy` bitset rather than a `HashSet`, because
/// [`Doc::active_inline_marks`] is called on every frame that draws a toolbar
/// and a set that allocates to answer "is Bold on?" is a set that shouldn't.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineMarks(u8);

impl InlineMarks {
    /// Every kind, in the order [`InlineMarks::iter`] yields them.
    const ALL: [InlineKind; 8] = [
        InlineKind::Strong,
        InlineKind::Emph,
        InlineKind::Verbatim,
        InlineKind::Mark,
        InlineKind::Superscript,
        InlineKind::Subscript,
        InlineKind::Insert,
        InlineKind::Delete,
    ];

    pub const fn empty() -> Self {
        InlineMarks(0)
    }

    /// Private: the set is an *answer*, and adding a mark to it doesn't mark
    /// anything ([`Doc::toggle`] does that). `FromIterator` is the way in.
    fn insert(&mut self, kind: InlineKind) {
        self.0 |= Self::bit(kind);
    }

    /// Flip `kind` in the set — the sticky-marks toggle at a collapsed caret.
    fn flip(&mut self, kind: InlineKind) {
        self.0 ^= Self::bit(kind);
    }

    /// The symmetric difference: which marks differ between the two sets. Used
    /// to resolve the marks already in force at the caret against the pending
    /// delta — a bit set in the delta flips the base mark for the next keystroke.
    fn xor(self, other: InlineMarks) -> InlineMarks {
        InlineMarks(self.0 ^ other.0)
    }

    /// Whether `kind` is in force — the toolbar's "is Bold active?".
    pub fn contains(self, kind: InlineKind) -> bool {
        self.0 & Self::bit(kind) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The marks in force, for a frontend that renders whatever is on rather
    /// than asking after a fixed list.
    pub fn iter(self) -> impl Iterator<Item = InlineKind> {
        Self::ALL.into_iter().filter(move |&k| self.contains(k))
    }

    fn bit(kind: InlineKind) -> u8 {
        1 << match kind {
            InlineKind::Strong => 0,
            InlineKind::Emph => 1,
            InlineKind::Verbatim => 2,
            InlineKind::Mark => 3,
            InlineKind::Superscript => 4,
            InlineKind::Subscript => 5,
            InlineKind::Insert => 6,
            InlineKind::Delete => 7,
        }
    }
}

impl FromIterator<InlineKind> for InlineMarks {
    fn from_iter<I: IntoIterator<Item = InlineKind>>(iter: I) -> Self {
        let mut m = InlineMarks::empty();
        for k in iter {
            m.insert(k);
        }
        m
    }
}

/// What kind of edit produced an undo group. Same-kind edits in a row coalesce
/// into one undo step (a run of typed characters undoes together); `Other` never
/// coalesces, so a paste, format toggle, or block change is always its own step.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    /// One step of an IME composition — see [`Doc::edit_composing`]. Its own kind
    /// rather than `Insert`'s because a composition is not typing: each step
    /// *replaces* the last (`か` → `かん` → `感`), so the run has to coalesce even
    /// though no two steps insert the same bytes, and it must not fold into the
    /// typed characters on either side of it.
    Compose,
    Other,
}

/// Which side of the caret a delete looks for an in-cell `<br>` break to swallow
/// whole — see [`Doc::cell_break_at`]. `Backward` is Backspace (a break ending at
/// the caret), `Forward` is Delete (one starting at it).
#[derive(Clone, Copy)]
enum BreakEdge {
    Backward,
    Forward,
}

/// A re-spelling of one inline mark run, held ready in case the edit about to
/// happen breaks it — see [`Doc::mark_edge_fix`] and [`Doc::repair_mark_edges`].
/// Every offset in it is in the coordinates the document will have *after* the
/// plain edit, since that is when it may be applied.
struct MarkEdgeFix {
    /// The run's kind, and an offset inside what was its content: together they
    /// answer "did the plain edit actually break this mark?" — the question that
    /// decides whether any of this is applied at all.
    kind: InlineKind,
    probe: usize,
    /// The byte range to re-spell (the run's delimiters included) and its new
    /// spelling, with the edge whitespace moved outside the delimiters.
    start: usize,
    end: usize,
    text: String,
    /// Where the caret belongs afterwards — the same place on screen it would
    /// have had, which is now on the other side of a delimiter.
    caret: usize,
    /// The marks in force for text typed at that caret. The caret can land
    /// outside a run it was inside, and the marks have to survive the move or
    /// the toolbar goes dark mid-word.
    want: InlineMarks,
}

/// The caret and selection at one moment — the part of a history step twig's
/// `Change` cannot carry, because the caret is leaf's state and twig only knows
/// about bytes. leaf serializes it into the opaque per-state blob twig now
/// stores in its own undo history (see `record_caret`), so undo and redo hand
/// back the caret that matches the source they restore.
#[derive(Clone, Copy)]
struct CaretState {
    caret: usize,
    anchor: Option<usize>,
}

impl CaretState {
    /// Pack into the fixed 17-byte blob leaf hands twig: the caret as a u64,
    /// then an anchor-present flag and the anchor. twig copies these bytes and
    /// never reads them.
    fn to_blob(self) -> [u8; 17] {
        let mut b = [0u8; 17];
        b[..8].copy_from_slice(&(self.caret as u64).to_le_bytes());
        if let Some(a) = self.anchor {
            b[8] = 1;
            b[9..].copy_from_slice(&(a as u64).to_le_bytes());
        }
        b
    }

    /// Recover a state from twig's blob, or `None` when it is empty or the wrong
    /// length — a state twig restored that never had a caret set on it, which
    /// leaves the caller to fall back to the edit site.
    fn from_blob(b: &[u8]) -> Option<Self> {
        let b: &[u8; 17] = b.try_into().ok()?;
        let caret = u64::from_le_bytes(b[..8].try_into().unwrap()) as usize;
        let anchor = (b[8] != 0).then(|| u64::from_le_bytes(b[9..].try_into().unwrap()) as usize);
        Some(CaretState { caret, anchor })
    }
}

/// A footnote reference and the note it names — the answer to
/// [`Doc::footnote_at`].
///
/// The two `Option`s move together: a reference whose definition is missing has
/// neither a body to show nor a place to jump to, and one that resolved has
/// both.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FootnoteRef {
    /// The reference's label — the `1` of `[^1]`, with neither the `^` that
    /// spells it a footnote nor the brackets around it.
    pub label: String,
    /// The note's body as source bytes (see
    /// [`wysiwyg::footnote_body_span`](crate::wysiwyg)), or `None` when the
    /// document defines no `[^label]:` to read one from.
    pub text: Option<String>,
    /// Where the note's *body* starts, for a "go to note" that moves the caret
    /// there. `None` alongside a `None` `text`.
    ///
    /// The body rather than the definition, because this is an offset to put a
    /// caret on and the `[^1]:` marker is decoration the caret can't occupy —
    /// aiming at the definition's first byte snaps to the nearest real stop,
    /// which is up in the paragraph above the note. It is also simply where a
    /// reader following a reference wants to land: at the note's first word,
    /// ready to read or amend it.
    pub offset: Option<usize>,
    /// Where the note's body ends, exclusive — so a frontend can ask which
    /// *rendered rows* the note occupies and draw those instead of [`text`](Self::text).
    ///
    /// The rows are the note with its markup resolved: `see *later*` reaches a
    /// frontend as an italic run, not as asterisks. `text` is the source bytes
    /// and stays the honest answer for anything that wants the note as written
    /// (a search index, a copy); this pair of offsets is for anything that wants
    /// it as *read*. `None` alongside a `None` `offset`.
    pub end: Option<usize>,
}

/// A footnote definition and the reference that sends a reader to it — the
/// answer to [`Doc::footnote_definition_at`], and the other half of the round
/// trip [`FootnoteRef`] starts.
///
/// A note is a place a reader *arrives*, so the useful thing to know while
/// standing in one is the way back. Without this the jump to a note is a
/// one-way door: the definitions sit at the foot of the document, so returning
/// by hand means scrolling back up and finding the sentence again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FootnoteDef {
    /// The definition's label — the `1` of `[^1]: …`, marker and colon stripped,
    /// spelled exactly as [`FootnoteRef::label`] spells the same footnote's.
    pub label: String,
    /// Where the reference's *label* is, for a "back to reference" that moves
    /// the caret there. `None` for a note nothing refers to — an orphan, which
    /// is worth being able to say rather than silently doing nothing.
    ///
    /// The label rather than the reference's first byte, for
    /// [`FootnoteRef::offset`]'s reason: a reference's brackets are decoration
    /// and its label is the only part of it the caret can rest on.
    ///
    /// The *first* reference, when a label is cited more than once: a repeated
    /// citation has no one true home, and the first is both the one a reader
    /// most likely came from and the only choice that doesn't depend on how
    /// they got here.
    pub offset: Option<usize>,
}

/// twig's cap on retained undo steps (`MAX_UNDO` in its splicer), which
/// [`Doc::can_undo`]'s count follows: past it twig drops the oldest step.
const UNDO_CAP: usize = 200;

/// An open [`Doc::begin_undo_group`]: how deeply it is nested, and whether an
/// edit made inside it has put its one step on the history yet.
#[derive(Clone, Copy, Debug)]
struct UndoGroup {
    depth: usize,
    has_step: bool,
}

/// What [`Doc::set_unrevealed`] sets aside: the screen's map, what it was
/// built from, and the caret state a paper build's clamp must not move.
struct ScreenMap {
    vmap: VisualMap,
    key: Option<(u64, Option<usize>, Option<Reveal>)>,
    caret: usize,
    anchor: Option<usize>,
    goal_col: Option<usize>,
}

/// Where a locator lands — the answer to [`Doc::locate`].
///
/// A locator (the `v2` of a `chapter.dj#v2`) names a *place* rather than a
/// document, and a place is a span rather than a point: a reader following one
/// wants the caret at its first byte, and a reader merely *peeking* at one wants
/// the block it covers drawn. Both are served by carrying the whole span, and
/// only one of the two can be recovered from an offset alone.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Landing {
    /// The first byte of the block the locator names — where a caret goes.
    pub start: usize,
    /// One past its last byte, so a frontend can map the pair through
    /// [`VisualMap::row_range_for`](crate::wysiwyg::VisualMap::row_range_for) to the rendered rows the block occupies and draw
    /// those, the way a footnote peek draws a note ([`FootnoteRef::end`]).
    pub end: usize,
}

/// The heading a place in the document sits under — the answer to
/// [`Doc::heading_at`].
///
/// What a host writing a link *to* a position needs: the `#its-slug` half of a
/// reference is made from `text`, and a peek at the target draws `span`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Heading {
    /// The heading's inline content with its markup stripped — `A *big* day`
    /// is `A big day` — which is what a slug is made from.
    pub text: String,
    /// 1 for `#`, 2 for `##`, and so on.
    pub level: u32,
    /// The heading block's own source span, marker and all — the heading, not
    /// the section it opens.
    pub span: Range<usize>,
}

/// Where a dragged block would land — the answer to [`Doc::drop_target_at`].
///
/// A drop is aimed at a *row* (the one under the pointer) and lands at a
/// *boundary* (between two blocks), and a frontend needs both halves: the
/// offset to hand [`Doc::move_block`], and the row to draw the indicator
/// above — which is not the row aimed at, since a drop on the lower half of a
/// block lands below it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DropTarget {
    /// The boundary offset — `move_block`'s `to`.
    pub offset: usize,
    /// The rendered row the indicator is drawn above; `rows.len()` for a drop
    /// below everything.
    pub row: usize,
}

/// A selection cited out of the source: the text itself, up to a requested
/// number of characters either side, and the byte range it came from. See
/// [`Doc::selection_quote`].
///
/// The prefix and suffix are what make the quote *re-findable*: the same text
/// can occur twice, and a little of what surrounded it is how a later reader —
/// or the same document after an edit — tells the occurrences apart. The Web
/// Annotation model calls this a `TextQuoteSelector`; the shape is older than
/// the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    /// The selected source, verbatim.
    pub exact: String,
    /// What immediately preceded it — possibly empty, at the document's start.
    pub prefix: String,
    /// What immediately followed it — possibly empty, at the document's end.
    pub suffix: String,
    /// Byte offset in the source where the selection begins.
    pub start: usize,
    /// Byte offset where it ends (exclusive).
    pub end: usize,
}

/// A host-painted range of the source — an annotation's footprint, a search
/// hit, a reviewer's mark. Leaf renders it (a background wash behind the
/// glyphs whose source falls inside it) and hands back the `id` when the
/// reader activates it; what the range *means* is entirely the host's.
///
/// Ranges are source bytes, like the caret and the selection, so a host that
/// anchors quotes against the source ([`Doc::selection_quote`] is the other
/// half of that loop) can paint what it found without any coordinate
/// conversion. A range that drifts off the text it meant is the host's to
/// re-anchor; leaf draws what it is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// Byte offset in the source where the wash begins.
    pub start: usize,
    /// Byte offset where it ends (exclusive).
    pub end: usize,
    /// The host's name for it, handed back on activation. Opaque to leaf.
    pub id: String,
    /// A rendering hint the frontend maps — a `#RRGGBB` hex string, or
    /// nothing for the theme's default wash.
    pub color: Option<String>,
    /// A margin glyph's name, or nothing for wash-only ink. A highlight with
    /// a marker gets a small glyph in the margin beside its first line, and
    /// the glyph — not the wash — is what activates it: the wash is ink, the
    /// marker is the control, which is what lets a reader put a caret in (or
    /// copy from) annotated text without a card leaping at them. The name is
    /// opaque to leaf; an Apple frontend reads it as an SF Symbol, a web one
    /// as a class.
    pub marker: Option<String>,
}

impl Highlight {
    /// The range covering source `offset` in a list [`Doc::set_highlights`]
    /// sorted, first by start where several overlap — the one place that
    /// question is answered, for the frontends that paint by asking it as well
    /// as for [`Doc::highlight_at`].
    ///
    /// The list is sorted by `(start, end)`, so the scan can stop at the first
    /// range starting past `offset` rather than running to the end. A painter
    /// asking once per glyph wants [`HighlightCursor`] instead; this is the
    /// one-shot form, for the host asking what the reader just activated.
    pub fn covering(highlights: &[Highlight], offset: usize) -> Option<&Highlight> {
        highlights
            .iter()
            .take_while(|h| h.start <= offset)
            .find(|h| offset < h.end)
    }
}

/// [`Highlight::covering`] for a caller walking the document in order — which
/// is every painter, since a frontend draws rows top to bottom and glyphs left
/// to right.
///
/// The one-shot form is a scan from the front of the list per glyph, and a
/// document with two hundred search hits pays that two hundred times a row. A
/// range that ends at or before an offset can never cover that offset *or any
/// later one*, so the cursor retires those permanently and each glyph costs the
/// ranges that actually reach it. The answer is identical to
/// [`Highlight::covering`]'s, offset for offset — this is the same scan with
/// the part that was being redone dropped, not a cheaper approximation.
///
/// Offsets are expected to arrive non-decreasing. One that goes backwards is
/// still answered correctly: the cursor re-seats to the front, since a painter
/// that revisits a row is asking a question the retired ranges may own again.
pub struct HighlightCursor<'a> {
    highlights: &'a [Highlight],
    /// The first range not yet retired.
    at: usize,
    /// The last offset asked about, to notice a caller going backwards.
    last: usize,
}

impl<'a> HighlightCursor<'a> {
    pub fn new(highlights: &'a [Highlight]) -> Self {
        HighlightCursor {
            highlights,
            at: 0,
            last: 0,
        }
    }

    /// The range covering `offset`, advancing the cursor past every range that
    /// can no longer cover anything.
    pub fn at(&mut self, offset: usize) -> Option<&'a Highlight> {
        if offset < self.last {
            self.at = 0;
        }
        self.last = offset;
        while self
            .highlights
            .get(self.at)
            .is_some_and(|h| h.end <= offset)
        {
            self.at += 1;
        }
        Highlight::covering(&self.highlights[self.at..], offset)
    }
}

/// The identity of a built [`VisualMap`] — see [`Doc::visual_key`]. Opaque on
/// purpose: the only useful question is whether two of them are the same map,
/// and what is behind it — which `Doc` built it, and the (revision, wrap,
/// reveal line) it was built from — is core's business.
///
/// The document is part of it because the rest is not unique to one: two
/// documents opened at the same width are both at revision zero with no reveal
/// line, and a frontend holding one copy of a map across the two would take
/// the second's key for the first's and paint the wrong document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VisualKey(u64, Option<(u64, Option<usize>, Option<Reveal>)>);

pub struct Doc {
    editor: Editor,
    pub format: Format,
    pub path: PathBuf,
    /// Current source, refreshed from the editor after every successful edit.
    pub source: String,
    /// The caret, as a byte offset into `source` (always on a char boundary).
    pub caret: usize,
    /// The selection's fixed end, if a selection is active; the moving end is
    /// the caret. `None` means no selection.
    pub anchor: Option<usize>,
    pub dirty: bool,
    pub status: Option<String>,
    pub view: View,
    /// Whether the document refuses to change — a *reading* surface over the
    /// same rendering, selection, and navigation the editor has.
    ///
    /// Enforced here rather than by each frontend hiding its input paths,
    /// because every mutation funnels through a few doors —
    /// [`splice_exact`](Self::splice_exact), [`undo`](Self::undo),
    /// [`redo`](Self::redo), and the handful of inserts that go to twig's own
    /// verbs directly rather than through the splice (a typed literal, a link,
    /// an image, a rule, a footnote, a cell's line break) — and guarded doors
    /// are a guarantee where a frontend's suppressed keyboard is a hope. A
    /// gated door reports exactly like a rolled-back splice, a path every
    /// caller already handles. `a_read_only_document_refuses_every_door` is
    /// the list; a new `self.editor.insert_*` call belongs on it.
    read_only: bool,
    /// The host-painted ranges, kept sorted by start — see [`Highlight`].
    /// State like the selection rather than like the text: no edit history,
    /// no dirty bit, redrawn from whatever the host last set.
    highlights: Vec<Highlight>,
    /// How much of the source markup the rich view exposes — a frontend preference (see
    /// [`MarkupMode`]). Its two axes are read apart: the rendering one by
    /// [`reveal_line`](Self::reveal_line), the editing one by
    /// [`insert`](Self::insert).
    markup_mode: MarkupMode,
    /// Whether soft breaks fold into the reflowed paragraph or render where
    /// they were written (see [`LineFlow`]) — an independent frontend
    /// preference the WYSIWYG builder consults when it lays out a block.
    line_flow: LineFlow,
    /// The kind of the last edit, for coalescing: twig owns the undo *history*
    /// (see `undo`/`redo`), but "what counts as one undo step" is a frontend-UX
    /// call, so leaf decides when a run continues and tells twig to coalesce.
    last_edit_kind: Option<EditKind>,
    /// The inline marks the user has toggled *at a collapsed caret* with no
    /// selection — "start typing bold here". Held as the XOR delta from the marks
    /// already in force at [`pending_at`](Self::pending_at): a set bit means
    /// "flip this kind for the next typed text", so it both turns a mark on where
    /// none is (type into bold) and off where one already covers the caret (type
    /// past the bold you're standing in). [`Doc::insert`] realises it onto the
    /// freshly typed text and then clears it — a mark once realised is carried by
    /// the caret sitting inside the run, not by this delta.
    pending_marks: InlineMarks,
    /// The caret offset [`pending_marks`](Self::pending_marks) applies to. The
    /// delta is live only while the caret still stands here with no selection;
    /// any motion or edit ([`move_to`](Self::move_to), a splice, a click) drops
    /// it, so a toggled-but-never-typed format doesn't leak onto text elsewhere.
    pending_at: Option<usize>,
    /// The source as of the last open/save — `dirty` is `source != clean_source`,
    /// so undoing back to the saved state correctly clears the modified flag.
    clean_source: String,
    /// A hash of the bytes leaf last read from `path` or wrote to it; `None`
    /// while the document has no file behind it. [`Doc::disk_state`] compares
    /// the file against this to catch an edit made *outside* leaf before a save
    /// silently overwrites it — `clean_source` only knows what leaf itself did.
    ///
    /// A hash, not an mtime: mtime is the cheap answer and the wrong one — two
    /// writes inside one filesystem timestamp tick are indistinguishable, a
    /// clock that steps backwards (or a writer that restores an mtime) hides a
    /// real change, and a `touch` invents one. The whole point of the watermark
    /// is to not clobber someone's work, so it reads the bytes and compares what
    /// is actually there. That costs a file read per question, which is why the
    /// question is asked on a user event (focus, save) and not every frame.
    disk_hash: Option<u64>,
    /// The "sticky" display column vertical motion aims for, in the active
    /// view's grid. Set on the first `move_up`/`move_down` of a run and
    /// reused by every subsequent one in that run, so passing through a
    /// shorter line doesn't permanently forget the original column. Any
    /// horizontal motion or edit clears it.
    ///
    /// A column, not a character index: dropping down a line of `你好` onto one
    /// of ASCII has to land under the glyph the caret was drawn beneath, which
    /// is the only thing the user can see to aim by. Where the goal falls inside
    /// a wide character on the target line, the mapping resolves it to that
    /// character — the caret lands on it rather than between its cells.
    goal_col: Option<usize>,
    /// The rendered map for the WYSIWYG view; empty in the source view. Movement
    /// and clicks read it to stay in visible space.
    pub vmap: VisualMap,
    /// The syntax map for the source view; empty in the WYSIWYG view, which
    /// styles resolved glyphs instead. Built by [`Doc::build_source`] — a
    /// frontend that never calls it paints raw source unstyled, which is what
    /// every frontend did before this map existed.
    pub smap: SourceMap,
    /// The revision `smap` was built from, or `None` before the first build.
    /// The map is a pure function of the text alone — no width, no caret, no
    /// reveal line — so unlike [`vmap_key`](Self::vmap_key) the revision is the
    /// whole key.
    smap_key: Option<u64>,
    /// Everything the map is built from, as one number: bumped whenever the
    /// document's text changes, and never by a motion, a selection, or a save.
    /// A frontend can hold work against it — see [`Doc::revision`].
    revision: u64,
    /// How many history steps stand behind the caret, and how many ahead of
    /// it — the answer to a native Edit menu's "may Undo be enabled?", which
    /// twig's history does not answer itself. A mirror of twig's two stacks,
    /// kept exact by following every move twig makes to them: a step onto the
    /// undo stack at [`refresh`](Self::refresh), the funnel every edit comes
    /// through, capped where twig caps it ([`UNDO_CAP`]); one fewer at each
    /// [`coalesce_last_undo`](Self::coalesce_last_undo), under twig's own
    /// two-step rule; and moved back and forth by [`undo`](Self::undo) and
    /// [`redo`](Self::redo). A counter that only ever went up — which this was
    /// — answered `true` after a coalesced run had been undone whole. Should
    /// it ever drift anyway, the first undo twig refuses sets it to zero.
    undo_steps: usize,
    redo_steps: usize,
    /// The undo group a host has open, if any — see
    /// [`begin_undo_group`](Self::begin_undo_group). `None` outside one.
    undo_group: Option<UndoGroup>,
    /// Make the next literal insert fail as twig would refuse one — a test's
    /// way to reach the rollback no document provokes.
    #[cfg(test)]
    refuse_next_literal: bool,
    /// What `vmap` was built from, or `None` before the first build. The map is
    /// a pure function of `(revision, wrap, reveal line)`, so when those haven't
    /// moved, rebuilding it produces the identical map — see
    /// [`Doc::build_visual`].
    ///
    /// The reveal line ([`Doc::reveal_line`]) is the caret's, and is `None` in
    /// every mode but [`MarkupMode::Full`] — so outside that mode the key is
    /// text and width alone, and a caret motion still rebuilds nothing.
    vmap_key: Option<(u64, Option<usize>, Option<Reveal>)>,
    /// The screen's map, set aside while [`Doc::set_unrevealed`] lays the
    /// document out as paper, and put back when it stops — `Some` exactly
    /// while the map in `vmap` is the page's.
    screen: Option<Box<ScreenMap>>,
    /// Which `Doc` this is, distinct from every other one built in this
    /// process. Folded into [`VisualKey`] so that a map stashed by a frontend
    /// can never be mistaken for another document's — see
    /// [`Doc::visual_key`]. Nothing else reads it.
    identity: u64,
    /// Per-block row cache backing the incremental rebuild: when the text
    /// changes, only the top-level blocks whose bytes moved are re-rendered and
    /// the rest are reused shifted (see [`wysiwyg::BlockCache`]). Persists across
    /// builds; a pure accelerator, so it's never read for correctness.
    block_cache: wysiwyg::BlockCache,
    /// What the frontend has said about itself — how tall its pictures came
    /// out, keyed by destination or by TeX, and whether it paints a picture in
    /// a line — set through [`Doc::set_media_rows`], [`Doc::set_math_rows`],
    /// [`Doc::set_directive_rows`] and [`Doc::set_inline_pictures`]. Core does no I/O and lays out in glyphs,
    /// so this is the only way it learns a height or a capability. Threaded
    /// into every build; a change drops both caches, since none of it is in a
    /// block's bytes.
    surface: wysiwyg::Surface,

    // View geometry the renderer stamps each frame, so mouse events can map a
    // screen cell back to a byte offset.
    pub scroll: usize,
    pub body_origin: (u16, u16),
    /// Width of the body rectangle last painted by the frontend. Zero means
    /// unknown (used by tests or a frontend that has not drawn yet).
    pub body_width: u16,
    pub body_height: u16,
    /// The caret as of the last frame drawn, or `None` before the first.
    ///
    /// Scrolling is the viewport's business, not the caret's: the view follows
    /// the caret when the caret *moves*, but a wheel that doesn't touch the
    /// caret has to be free to scroll away from it — otherwise the view is
    /// pinned to the caret and stops dead at the edge of the document you can
    /// see. Comparing against this is what tells the two apart, and it catches a
    /// caret set by any route, including a frontend assigning the field itself.
    pub drawn_caret: Option<usize>,
}

/// The Markdown extensions every leaf document is parsed with — five of them,
/// each departing from twig's defaults for a reason leaf can state.
///
/// `html_elements` promotes embedded raw HTML (`<img>`, `<picture>`,
/// `<source>`, …) into semantic AST nodes, so a picture becomes a real `image`
/// node the frontends can frame and rasterize instead of opaque `raw_block`
/// text. `directives` turns on generic `:::name{.class}` fenced-div containers
/// (`directive` nodes), which a host app uses for its own semantics (diaryx's
/// `:::vis{.audience}` visibility blocks) — core renders any directive as a
/// plain tinted container, agnostic of `name`.
///
/// `highlight` and `highlight_colors` are the pair that makes Markdown read
/// `==text==` as a `mark` node, and `==🔴 text==` as one carrying a
/// `data-color`. leaf already had somewhere to put both: the
/// [`Mark`](crate::Role::Mark) role and the ⌘⇧M highlight button predate them,
/// and until twig 3.3 a Markdown document could only ever *receive* a highlight
/// from a Djot one it was converted from — the button wrote `==…==` and the
/// reparse read it straight back as text.
/// They are on together because a colour is inert without the highlight itself,
/// and a document that writes `==🔴 x==` means the colour by it.
///
/// `math` makes Markdown read `$…$` as an `inline_math` node and `$$…$$` as
/// a `display_math` one — what djot reads natively and what leaf has
/// somewhere to put: a formula typesets to a picture, or reveals to its TeX
/// on the caret's line. Without it a `$$` block is a paragraph whose `\,`
/// twig has already read as an escaped comma, and an author who types a
/// backslash in it is authoring Markdown, not TeX. The flag is bounded by
/// twig's own rule that a dollar followed by whitespace never opens math, so
/// `$5 and $6` stays prose.
///
/// Every flag is inert for non-Markdown formats, so it's safe to pass them
/// unconditionally. Threading this through every constructor (not just `open`)
/// keeps `from_source`, `blank`, and `reload` parsing the same document the same
/// way — twig reparses with these same flags after each edit.
pub(crate) fn parse_extensions() -> MarkdownExtensions {
    MarkdownExtensions {
        html_elements: true,
        directives: true,
        highlight: true,
        highlight_colors: true,
        math: true,
    }
}

/// Build an editor over `bytes` in `format` with leaf's [`parse_extensions`],
/// mapping twig's error into the `anyhow` context every constructor shares.
fn new_editor(bytes: &[u8], format: Format) -> Result<Editor> {
    Editor::new_ext(bytes, format, parse_extensions()).map_err(|e| anyhow!("twig parse: {e}"))
}

/// Does `format` spell a table as a **pipe table** — the one grid twig's table
/// editor knows how to emit?
///
/// This is the single capability leaf still has to answer for itself, and the
/// only hand-maintained format list left in this file. Every other gesture is
/// [`Format::supports`], which is twig's own answer read across the C ABI — but
/// twig deliberately leaves the table ops out of that query, because they read
/// no `Syntax` table at all. They rewrite a grid that is already in the source
/// and refuse on *position*, never on format. Handed a caret inside an HTML
/// `<table>`, `table_insert_row` therefore re-emits the whole element as
/// `| a | b |` and reports success — a real splice, a clean reparse, an honest
/// `dirty` flag, and nothing downstream able to tell it from a good edit.
///
/// So the list is narrow on purpose. `Format` is `#[non_exhaustive]`, and the
/// wildcard answers "no" for a format leaf has never heard of: a new twig
/// language that *does* spell pipe tables loses its grid controls until this
/// line is updated, which shows up as a missing button. The other default hands
/// it to [`Doc::table_op`], which rewrites documents it cannot spell.
fn spells_pipe_tables(format: Format) -> bool {
    matches!(format, Format::Markdown | Format::Djot)
}

/// Which of leaf's authoring controls this document's format can actually
/// spell — one flag per toolbar button, resolved once so a frontend can build
/// its chrome instead of discovering each refusal on a click.
///
/// Every field but [`table`](Self::table) is `Format::supports_with` on the
/// gesture the matching [`Doc`] method calls, so this record cannot drift from
/// what the ops do; `table` is [`spells_pipe_tables`], the one answer twig
/// doesn't export.
///
/// `supports_with` rather than `supports` because two of these are facts about
/// the *parse options* as much as about the format. `Format::supports` answers
/// for twig's defaults, and leaf never parses with those — it parses with
/// [`parse_extensions`], and a document's toolbar has to describe the document
/// it is over. A Markdown editor holding `highlight` authors `==text==`; one
/// without it would mint bytes its own reparse hands back as plain text, which
/// is why twig asks before it writes.
///
/// **The formats are ragged, and that is the point.** A single per-document
/// boolean was enough while the two authorable formats were Markdown and djot
/// and everything else spelled nothing. HTML is neither: it writes seven of the
/// eight inline marks as a tag pair, plus `<code>`, `<hr>`, an in-cell
/// `<br>`, and — since twig 3.4 — a heading or paragraph rebuilt as its tag
/// pair, and since 3.5 a quote, a list, a code block, a link and an image
/// printed as fresh nodes; it spells no task box (a form control there) and
/// no footnote, and its `<table>` is one twig reads but will not write. So
/// ⌘B, ⌘1 and the quote button work in an HTML document and the task and
/// table buttons do not, and no one flag can say that. Markdown and djot
/// differ from each other too:
/// `^superscript^` is djot-only, and an in-cell `<br>` is Markdown-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    /// ⌘B — `InlineKind::Strong`.
    pub bold: bool,
    /// ⌘I — `InlineKind::Emph`.
    pub italic: bool,
    /// Inline code — `InlineKind::Verbatim`.
    pub code: bool,
    /// Highlight — `InlineKind::Mark`. Djot spells it, and so does Markdown
    /// under the `highlight` extension [`parse_extensions`] turns on: the
    /// button writes `==text==`, which is what the reparse reads back.
    pub mark: bool,
    /// ⌘U — `InlineKind::Insert`, which every format that marks at all spells.
    pub underline: bool,
    /// Strikethrough — `InlineKind::Delete`. Markdown spells GFM's `~~text~~`
    /// out of the box, since twig parses it out of the box.
    pub strike: bool,
    /// The highlight *palette* — [`Doc::set_mark_color`]. Narrower than
    /// [`mark`](Self::mark) and deliberately its own flag: Markdown spells a
    /// colour on a highlight (`==🔴 text==`) and djot spells only the highlight,
    /// so a toolbar offering the swatches wherever the button lights would offer
    /// them in a document that cannot write one. Pair with
    /// [`Doc::caret_in_mark`], which asks the other question — the palette needs
    /// a highlight to colour as much as a format that spells one.
    pub mark_color: bool,
    pub superscript: bool,
    pub subscript: bool,
    /// Heading levels and "make this a paragraph" — [`Doc::set_block`].
    pub heading: bool,
    pub blockquote: bool,
    pub bullet_list: bool,
    pub ordered_list: bool,
    /// The checkbox controls: giving an item a box, and ticking one.
    pub task: bool,
    pub link: bool,
    /// Covers [`Doc::insert_media`] too — see the note there on why the three
    /// media kinds stand or fall together.
    pub image: bool,
    /// The horizontal-rule button. HTML spells this one (`<hr>`).
    pub thematic_break: bool,
    /// The footnote button — [`Doc::insert_footnote`]. Markdown and djot spell
    /// the pair; HTML has no footnote of its own, so the button goes away rather
    /// than writing brackets that would render as brackets.
    pub footnote: bool,
    /// The code-block button — [`Doc::toggle_code_block`], twig's
    /// `Gesture::ToggleCodeBlock`. Markdown and djot spell the fence; HTML
    /// rebuilds the block as `<pre><code>`.
    pub code_block: bool,
    /// Setting a fenced block's language — a control only ever offered with the
    /// caret already in a fence.
    pub code_language: bool,
    /// The grid controls: insert/delete/move a row or column, set a column's
    /// alignment. Pair with [`Doc::caret_in_table`], which asks the other
    /// question — an HTML `<table>` holds the caret and still can't be edited.
    pub table: bool,
    /// Shift+Return inside a cell. Markdown and HTML spell it; djot has no
    /// idiomatic in-cell break.
    pub cell_line_break: bool,
    /// The alignment control — [`Doc::set_alignment`], twig's
    /// `Gesture::SetBlockAttrs`. Every format leaf opens but XML spells a
    /// block's attributes, Markdown under the `html_elements`
    /// [`parse_extensions`] turns on (a `<div>` around the block) and AsciiDoc
    /// through its `[…]` line.
    pub alignment: bool,
    /// The line-spacing menu — [`Doc::set_line_spacing`]. The same gesture as
    /// [`alignment`](Self::alignment) and so the same answer, and its own flag
    /// because a toolbar dims controls one at a time and the pair may yet
    /// diverge.
    pub line_spacing: bool,
    /// The size menu — [`Doc::set_font_size`], twig's `Gesture::WrapRangeAttrs`
    /// over a selection. **Narrower than the block pair**: AsciiDoc's
    /// `[#id.role]#text#` keeps an id and a role and has no slot for a
    /// `data-` key, so twig refuses the span there and this is `false` while
    /// [`alignment`](Self::alignment) is `true`. The block-level form of the
    /// same property — the caret in a paragraph, no selection — goes through
    /// `SetBlockAttrs` and still works, which is why the flag describes the
    /// control rather than the caret.
    pub font_size: bool,
    /// The face menu — [`Doc::set_font_family`]. `WrapRangeAttrs`, as
    /// [`font_size`](Self::font_size) is.
    pub font_family: bool,
    /// The text-colour swatches — [`Doc::set_text_color`]. `WrapRangeAttrs`,
    /// and not to be confused with [`mark_color`](Self::mark_color): that is a
    /// highlight's background and rides the `mark` node twig already owns,
    /// this is a run's foreground and rides an attributed span.
    pub text_color: bool,
    /// The page-break button — [`Doc::insert_page_break`], twig's
    /// `Gesture::InsertDirective`. Markdown under the `directives` extension
    /// [`parse_extensions`] turns on (`::page-break`), djot, which spells it
    /// as an empty `::: page-break` fence, HTML (`<page-break></page-break>`)
    /// and AsciiDoc (`<<<`) — each drawn as the same placeholder row.
    pub page_break: bool,
    /// The directives a host names — [`Doc::insert_directive`], twig's
    /// `Gesture::InsertDirective` for any name. **Narrower than
    /// [`page_break`](Self::page_break)**, which is the same gesture: twig
    /// spells an arbitrary directive in HTML (a custom element) and AsciiDoc
    /// (an open block with a style), and the walker reads back neither as a
    /// directive — it reads their spellings of `page-break` and no other name.
    /// So this is Markdown under the `directives` extension and djot, the two
    /// formats where what is written comes back in
    /// [`VisualMap::directives`](crate::wysiwyg::VisualMap::directives) with
    /// its name. djot has no leaf directive label: see
    /// [`Doc::insert_directive`].
    pub directives: bool,
    /// Moving a block — [`Doc::move_block`] and the Alt+↑/↓ pair, twig's
    /// `Gesture::MoveBlock`. Every format with blocks a caret can name; XML
    /// has none.
    pub move_block: bool,
    /// Verse — [`Doc::toggle_verse`], twig's `Gesture::ToggleLineBlock`. A
    /// block of lines whose breaks are the content: Markdown's
    /// `<div class="verse">` (under the `html_elements` extension leaf always
    /// parses with), djot's `::: verse`, AsciiDoc's `[verse]`. HTML's spelling
    /// reads back as a plain container, so it answers `false`.
    pub verse: bool,
}

impl Capabilities {
    /// Resolve every flag for `format`, as leaf parses it. Pure and cheap —
    /// twig computes each from a static table — but a frontend that wants to
    /// hold them can.
    ///
    /// The extensions are not a parameter because they are not a choice a
    /// caller makes: every leaf document is parsed with [`parse_extensions`],
    /// so the format is the whole of what varies.
    pub fn of(format: Format) -> Self {
        let exts = parse_extensions();
        let supports = |g| format.supports_with(exts, g);
        let inline = |k| supports(Gesture::ToggleInline(k));
        let container = |k| supports(Gesture::ToggleBlockContainer(k));
        Self {
            bold: inline(InlineKind::Strong),
            italic: inline(InlineKind::Emph),
            code: inline(InlineKind::Verbatim),
            mark: inline(InlineKind::Mark),
            underline: inline(InlineKind::Insert),
            strike: inline(InlineKind::Delete),
            mark_color: supports(Gesture::SetMarkColor),
            superscript: inline(InlineKind::Superscript),
            subscript: inline(InlineKind::Subscript),
            heading: supports(Gesture::SetBlock),
            blockquote: container(BlockContainerKind::BlockQuote),
            bullet_list: container(BlockContainerKind::BulletList),
            ordered_list: container(BlockContainerKind::OrderedList),
            // Both halves of the checkbox story, and leaf offers no control that
            // needs only one: the item gesture mints the box, the checked one
            // ticks it, and a format spelling a `task_marker` spells both.
            task: supports(Gesture::ToggleTaskItem) && supports(Gesture::ToggleTaskChecked),
            link: supports(Gesture::InsertLink),
            image: supports(Gesture::InsertImage),
            thematic_break: supports(Gesture::InsertThematicBreak),
            footnote: supports(Gesture::InsertFootnote),
            code_block: supports(Gesture::ToggleCodeBlock),
            code_language: supports(Gesture::SetCodeLanguage),
            table: spells_pipe_tables(format),
            cell_line_break: supports(Gesture::InsertLineBreak),
            // The presentation vocabulary, one gesture per level: the two
            // line-level properties are a block's attributes and the three
            // run-level ones a span's. They are asked separately because the
            // formats answer differently — AsciiDoc spells the block and not
            // the span — and a toolbar that dimmed all five together would dim
            // three controls that work.
            alignment: supports(Gesture::SetBlockAttrs),
            line_spacing: supports(Gesture::SetBlockAttrs),
            font_size: supports(Gesture::WrapRangeAttrs),
            font_family: supports(Gesture::WrapRangeAttrs),
            text_color: supports(Gesture::WrapRangeAttrs),
            // Every format twig spells the directive in: the walker draws
            // HTML's `<page-break>` and AsciiDoc's `<<<` as the same
            // placeholder Markdown's `::page-break` and djot's fence get.
            page_break: supports(Gesture::InsertDirective),
            // Only where the walker reads an arbitrary name back — see the
            // field. twig's own answer is also `true` for HTML and AsciiDoc.
            directives: supports(Gesture::InsertDirective)
                && matches!(format, Format::Markdown | Format::Djot),
            move_block: supports(Gesture::MoveBlock),
            verse: supports(Gesture::ToggleLineBlock),
        }
    }
}

/// The source of [`Doc::identity`], one per document ever built.
static NEXT_IDENTITY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests {

    use super::*;
    use crate::style::{FontFamily, LineSpacing, SizeStep};

    /// A document open in `view`. WYSIWYG motion reads the visual map, which the
    /// renderer stamps each frame, so the map is built here too — a WYSIWYG doc
    /// without one is a view no user is ever in.
    fn doc_in(view: View, name: &str, body: &str) -> Doc {
        // The fixture name doubles as the temp file's, so two tests picking the
        // same one raced under the parallel runner and read each other's body —
        // a green suite proving the wrong thing. The counter makes that
        // unreachable rather than asking every future caller to notice.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_test_{name}_{seq}.md"));
        std::fs::write(&p, body).unwrap();
        let mut d = Doc::open(p).unwrap();
        d.view = view;
        if view == View::Wysiwyg {
            d.build_visual(80);
        }
        d
    }

    // Source-view document for the source-behaviour tests. `Doc::open` now
    // defaults to WYSIWYG (leaf's default view), so pin the source view here;
    // `wysiwyg_doc` builds the rich-text variant on top of this.
    fn doc_with(name: &str, body: &str) -> Doc {
        doc_in(View::Source, name, body)
    }

    /// Every visual row's drawn text — what the reader actually sees, which is
    /// the only thing the reveal preference is supposed to change.
    fn drawn_rows(d: &Doc) -> Vec<String> {
        d.vmap
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect()
    }

    /// Put the caret at the first byte of `needle` and rebuild, so the row under
    /// it becomes the revealed line.
    fn caret_at(d: &mut Doc, needle: &str) {
        d.caret = d.source.find(needle).expect("needle in source");
        d.build_visual(80);
    }

    #[test]
    fn blockquote_after_a_list_is_not_bulleted() {
        // twig nests a following top-level block quote under the `bullet_list`
        // (a direct child, not a `list_item`). The map must render it de-nested —
        // `│ quote`, never `• │ quote` — with a blank separator, like any block
        // that follows a list. Regression for the "combined list + blockquote" bug.
        let mut d = doc_in(View::Wysiwyg, "bq_after_list", "- item\n\n> quote\n");
        d.build_visual(80);
        let rows: Vec<String> = d
            .vmap
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect();
        assert!(
            rows.iter().any(|r| r == "│ quote"),
            "block quote should render on its own gutter, got rows: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains('•') && r.contains('│')),
            "no row should carry both a bullet and a quote gutter, got rows: {rows:?}"
        );
    }

    // ── the map is built at most once per (revision, wrap) ───────────────────
    //
    // A frontend repaints for reasons that have nothing to do with the text — a
    // blinking caret, a scroll — and rebuilding the map is O(document). These
    // pin *that the cache fires*, which a passing suite can't tell you: a cache
    // that never hits is invisible to every other test in this file.
    //
    // The probe is to wreck the built map and ask for it again. A rebuild
    // repairs it; a cache hit hands the wreckage straight back. Nothing else
    // can distinguish the two from outside.

    #[test]
    fn a_rebuild_with_nothing_changed_reuses_the_map() {
        let mut d = doc_in(View::Wysiwyg, "cache_hit", "# Title\n\nbody\n");
        d.build_visual(80);
        assert!(!d.vmap.rows.is_empty());
        d.vmap.rows.clear(); // wreck it
        d.build_visual(80);
        assert!(
            d.vmap.rows.is_empty(),
            "the map was rebuilt though nothing changed — the cache never fired"
        );
    }

    #[test]
    fn an_edit_rebuilds_the_map() {
        let mut d = doc_in(View::Wysiwyg, "cache_edit", "# Title\n\nbody\n");
        d.build_visual(80);
        let before = d.revision();
        d.vmap.rows.clear();
        d.insert("x");
        d.build_visual(80);
        assert!(d.revision() > before, "an edit must move the revision");
        assert!(
            !d.vmap.rows.is_empty(),
            "an edited document must not paint from a stale map"
        );
    }

    #[test]
    fn a_width_change_rebuilds_the_map() {
        // The map is a function of the wrap width too, so a resize is a miss
        // even though the text is untouched.
        let mut d = doc_in(
            View::Wysiwyg,
            "cache_width",
            "one two three four five six\n",
        );
        d.build_visual(80);
        d.vmap.rows.clear();
        d.build_visual(12);
        assert!(!d.vmap.rows.is_empty(), "a resize must rebuild the map");
        // And the unwrapped map is its own key, not the same as any width.
        d.vmap.rows.clear();
        d.build_visual_unwrapped();
        assert!(!d.vmap.rows.is_empty(), "unwrapped is a different map");
    }

    #[test]
    fn a_motion_does_not_rebuild_the_map() {
        // The whole point: moving the caret changes nothing the map is built
        // from. If a motion bumped the revision, every arrow key would cost a
        // full rebuild and the cache would be worthless.
        let mut d = doc_in(View::Wysiwyg, "cache_motion", "# Title\n\nbody text\n");
        d.build_visual(80);
        let rev = d.revision();
        d.move_right(false);
        d.move_right(true);
        d.move_down(false);
        assert_eq!(d.revision(), rev, "a motion must not move the revision");
        d.vmap.rows.clear();
        d.build_visual(80);
        assert!(
            d.vmap.rows.is_empty(),
            "a motion should not rebuild the map"
        );
    }

    #[test]
    fn saving_does_not_rebuild_the_map() {
        // Saving changes `dirty`, not the text.
        let mut d = doc_in(View::Wysiwyg, "cache_save", "# Title\n\nbody\n");
        d.insert("x");
        d.build_visual(80);
        let rev = d.revision();
        d.save();
        assert_eq!(d.revision(), rev, "a save must not move the revision");
        assert!(!d.dirty, "the save should have cleaned the document");
    }

    #[test]
    fn a_reload_rebuilds_the_map() {
        // Reload replaces the text without going through `refresh`, so it has to
        // move the revision itself — else the editor paints the old file.
        let mut d = doc_in(View::Wysiwyg, "cache_reload", "# Title\n\nbody\n");
        d.build_visual(80);
        let rev = d.revision();
        std::fs::write(&d.path, "# Other\n\nwholly new\n").unwrap();
        d.reload();
        assert!(d.revision() > rev, "a reload must move the revision");
        d.build_visual(80);
        let text: String = d
            .vmap
            .rows
            .iter()
            .flat_map(|r| r.glyphs.iter().map(|g| g.ch))
            .collect();
        assert!(
            text.contains("wholly new"),
            "the reloaded text should be on screen, got {text:?}"
        );
    }

    // ── golden-case harness ──────────────────────────────────────────────────
    // The pattern the whole parity suite can reuse: write a fixture with the
    // caret marked by `|`, run one action, and compare the rendered result —
    // also caret-marked — against the expected string. One readable line per
    // behavior, and it exercises the exact `Doc` ops both frontends call.

    /// Split a `|`-marked fixture into `(source, caret_offset)`.
    fn parse_caret(marked: &str) -> (String, usize) {
        let caret = marked.find('|').expect("fixture needs a `|` caret marker");
        (marked.replacen('|', "", 1), caret)
    }

    /// Render a doc's source with `|` at the caret (and `[`…`]` around any
    /// selection) so a result reads like the fixtures.
    fn render_caret(d: &Doc) -> String {
        // (offset, rank, char); rank keeps coincident markers ordered `[ | ]`
        // so the caret always renders inside its own selection.
        let mut marks: Vec<(usize, u8, char)> = vec![(d.caret, 1, '|')];
        if let Some((s, e)) = d.selection() {
            marks.push((s, 0, '['));
            marks.push((e, 2, ']'));
        }
        // Insert right-to-left: descending offset, then descending rank.
        marks.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let mut out = d.source.clone();
        for (at, _, ch) in marks {
            out.insert(at, ch);
        }
        out
    }

    /// Load a `|`-marked fixture, run `action`, return the caret-marked result.
    fn golden(name: &str, marked: &str, action: impl FnOnce(&mut Doc)) -> String {
        golden_in(View::Source, name, marked, action)
    }

    /// [`golden`] in a chosen view — the editing ops are the view's to share, so
    /// the same fixture has to read the same way in both.
    fn golden_in(view: View, name: &str, marked: &str, action: impl FnOnce(&mut Doc)) -> String {
        let (src, caret) = parse_caret(marked);
        let mut d = doc_in(view, name, &src);
        d.caret = caret;
        action(&mut d);
        render_caret(&d)
    }

    #[test]
    fn word_motion_walks_word_by_word() {
        let g = |m, f: fn(&mut Doc)| golden("word_motion", m, f);
        assert_eq!(
            g("hello wor|ld", |d| d.move_word_left(false)),
            "hello |world"
        );
        assert_eq!(
            g("hello| world", |d| d.move_word_left(false)),
            "|hello world"
        );
        assert_eq!(
            g("hel|lo world", |d| d.move_word_right(false)),
            "hello| world"
        );
        assert_eq!(
            g("hello| world", |d| d.move_word_right(false)),
            "hello world|"
        );
        // Punctuation is its own class, so motion stops at the boundary.
        assert_eq!(g("|foo.bar", |d| d.move_word_right(false)), "foo|.bar");
    }

    #[test]
    fn word_motion_extends_the_selection_when_asked() {
        assert_eq!(
            golden("word_sel", "hello |world", |d| d.move_word_right(true)),
            "hello [world|]"
        );
    }

    #[test]
    fn delete_word_removes_a_whole_word() {
        let g = |m, f: fn(&mut Doc)| golden("del_word", m, f);
        assert_eq!(g("hello world|", |d| d.delete_word_back()), "hello |");
        assert_eq!(g("hello |world", |d| d.delete_word_forward()), "hello |");
        assert_eq!(g("foo |bar baz", |d| d.delete_word_back()), "|bar baz");
    }

    // ── Home / End ───────────────────────────────────────────────────────────

    #[test]
    fn home_toggles_between_the_line_s_text_and_its_margin() {
        // Source: the indentation is what the toggle is for. WYSIWYG resolves an
        // indent to the markup it spells everywhere it means one, so the fixture
        // with whitespace left to walk is a code block, which is verbatim.
        let g = |m, f: fn(&mut Doc)| golden("smart_home", m, f);
        assert_eq!(g("    inden|ted", |d| d.move_home(false)), "    |indented");
        assert_eq!(g("    |indented", |d| d.move_home(false)), "|    indented");
        assert_eq!(g("|    indented", |d| d.move_home(false)), "    |indented");
        // A line with no indentation has one place to go, so the toggle is a
        // no-op rather than a trip to nowhere.
        assert_eq!(g("hel|lo", |d| d.move_home(false)), "|hello");
        assert_eq!(g("|hello", |d| d.move_home(false)), "|hello");

        let mut d = wysiwyg_doc("smart_home_wys", "```\n    indented\n```\n");
        let indent = d.source.find("    indented").unwrap();
        d.caret = indent + 6; // inside "indented"
        d.move_home(false);
        assert_eq!(
            d.caret,
            indent + 4,
            "wysiwyg: Home aims at the code line's text"
        );
        d.move_home(false);
        assert_eq!(
            d.caret, indent,
            "wysiwyg: the second press takes the indent"
        );
        d.move_home(false);
        assert_eq!(d.caret, indent + 4, "wysiwyg: the toggle swaps back");
    }

    #[test]
    fn end_takes_the_line_the_view_is_showing() {
        // The line differs by view for the same document, and that is the point:
        // a bare newline inside a paragraph is a soft break, which WYSIWYG draws
        // as a space on one row and the source view as two lines.
        let mut d = doc_with("end_src", "one two\nthree\n");
        d.caret = 1;
        d.move_end(false);
        assert_eq!(d.caret, 7, "source: the end of the source line");

        let mut d = wysiwyg_doc("end_wys", "one two\nthree\n");
        d.caret = 1;
        d.move_end(false);
        assert_eq!(
            d.caret, 13,
            "wysiwyg: the end of the row, soft break and all"
        );
    }

    #[test]
    fn home_and_end_extend_the_selection_when_asked() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("home_end_ext_{tag}"), "hello world");
            d.caret = 6;
            d.move_end(true);
            assert_eq!(d.selection(), Some((6, 11)), "{tag}: End extends");
            let mut d = doc_in(view, &format!("home_ext_{tag}"), "hello world");
            d.caret = 6;
            d.move_home(true);
            assert_eq!(d.selection(), Some((0, 6)), "{tag}: Home extends");
        }
    }

    // ── kill to the line's start / end ───────────────────────────────────────

    #[test]
    fn kill_to_the_line_start_and_end_in_both_views() {
        for (view, tag) in VIEWS {
            // The gap that reads as a paragraph break in each view: the source
            // view's lines are the renderer's rows only where the source says so.
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let mut d = doc_in(
                view,
                &format!("kill_end_{tag}"),
                &format!("one two{gap}three\n"),
            );
            d.caret = 3;
            d.delete_to_line_end();
            assert_eq!(
                d.source,
                format!("one{gap}three\n"),
                "{tag}: ^K to the line's end"
            );
            assert_eq!(d.caret, 3, "{tag}: the caret stays where it kills from");

            let mut d = doc_in(
                view,
                &format!("kill_start_{tag}"),
                &format!("one two{gap}three\n"),
            );
            d.caret = 7; // the end of the first line
            d.delete_to_line_start();
            assert_eq!(
                d.source,
                format!("{gap}three\n"),
                "{tag}: ⌘⌫ to the line's start"
            );
            assert_eq!(d.caret, 0, "{tag}");
        }
    }

    #[test]
    fn a_kill_at_the_line_s_edge_leaves_the_lines_joined() {
        // The decision: at the boundary both kills do nothing, rather than
        // eating the line break. "Line" is the view's own — in WYSIWYG it ends
        // at a soft wrap as often as at a newline, where there is nothing
        // written to delete — and a source newline is only half of the blank
        // line between two paragraphs, so taking it leaves a soft break rather
        // than the join it looks like. Backspace and Delete are the keys for it.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("one{gap}three\n");
            let mut d = doc_in(view, &format!("kill_edge_end_{tag}"), &src);
            d.caret = 3; // the end of "one"
            d.delete_to_line_end();
            assert_eq!(
                d.source, src,
                "{tag}: ^K at the line's end joined it to the next"
            );

            let mut d = doc_in(view, &format!("kill_edge_start_{tag}"), &src);
            d.caret = 3 + gap.len(); // the start of "three"
            d.delete_to_line_start();
            assert_eq!(
                d.source, src,
                "{tag}: ⌘⌫ at the line's start joined it to the last"
            );
        }
    }

    #[test]
    fn a_kill_takes_the_selection_when_there_is_one() {
        // What every other delete here does with one, so these two as well.
        for (view, tag) in VIEWS {
            for (name, kill) in [
                (
                    "end",
                    (|d: &mut Doc| d.delete_to_line_end()) as fn(&mut Doc),
                ),
                ("start", |d: &mut Doc| d.delete_to_line_start()),
            ] {
                let mut d = doc_in(view, &format!("kill_sel_{name}_{tag}"), "one two three\n");
                d.anchor = Some(4);
                d.caret = 7; // "two"
                kill(&mut d);
                assert_eq!(
                    d.source, "one  three\n",
                    "{tag}: {name} ignored the selection"
                );
                assert_eq!(d.selection(), None, "{tag}: {name}");
            }
        }
    }

    #[test]
    fn a_kill_takes_the_markup_it_empties_with_it() {
        // The same hazard a word-delete has: a WYSIWYG range covers what the
        // user can see, which for `**bold**` is the word and never the
        // delimiters, so a kill that stopped at the text would leave `a ****` —
        // markup wrapped around nothing.
        let mut d = wysiwyg_doc("kill_widen", "a **bold**\n");
        d.caret = d.source.find("bold").unwrap();
        d.delete_to_line_end();
        assert_eq!(d.source, "a \n");
    }

    #[test]
    fn a_kill_is_undone_in_one_step() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("kill_undo_{tag}"), "one two three\n");
            d.caret = 3;
            d.delete_to_line_end();
            assert_eq!(d.source, "one\n", "{tag}");
            d.undo();
            assert_eq!(d.source, "one two three\n", "{tag}: a kill takes one undo");
        }
    }

    #[test]
    fn select_block_grabs_the_whole_paragraph_from_any_wrapped_row() {
        // Regression: triple-click used move_home/move_end over visual rows, so
        // it only worked on a paragraph's first row (a wrap-boundary offset maps
        // to the earlier row). select_block_at reads the AST, so every offset in
        // the paragraph selects the whole thing.
        let body = "one two three four five six seven eight\n";
        let mut d = doc_with("sel_block", body);
        d.view = View::Wysiwyg;
        d.build_visual(12); // force the paragraph to wrap into several rows
        assert!(d.vmap.num_rows() > 1, "test needs a wrapped paragraph");
        let para = (0, "one two three four five six seven eight".len());
        for off in [0usize, 8, 19, 28, 38] {
            d.caret = 0;
            d.anchor = None;
            d.select_block_at(off);
            assert_eq!(
                d.selection(),
                Some(para),
                "offset {off} should select the paragraph"
            );
        }
    }

    #[test]
    fn select_block_uses_content_span_for_a_heading() {
        let mut d = doc_with("sel_head", "# Title\n\nbody\n");
        d.select_block_at(4); // inside "Title"
        // content_span excludes the "# " marker.
        assert_eq!(d.selected_text(), Some("Title"));
        d.select_block_at(10); // inside "body"
        assert_eq!(d.selected_text(), Some("body"));
    }

    #[test]
    fn select_all_spans_the_document() {
        let mut d = doc_with("sel_all", "abc\n\ndef\n");
        d.select_all();
        assert_eq!(d.selection(), Some((0, d.source.len())));
    }

    #[test]
    fn select_word_at_picks_the_surrounding_word() {
        let mut d = doc_with("sel_word", "hello world\n");
        d.select_word_at(8); // inside "world"
        assert_eq!(d.selection(), Some((6, 11)));
        // Double-clicking at end-of-word still grabs the word to its left.
        d.select_word_at(5); // the space between the words
        assert_eq!(d.selection(), Some((5, 6)));
    }

    #[test]
    fn word_at_names_the_word_without_selecting_it() {
        let d = doc_with("word_at", "hello café, world\n");
        assert_eq!(d.word_at(1), Some(0..5));
        // The boundary after a word still means it: a point on the trailing
        // half of its last letter hit-tests there.
        assert_eq!(d.word_at(5), Some(0..5));
        // Multi-byte letters are word characters, and the range ends on a
        // character boundary; an offset inside `é` is the word it is in.
        assert_eq!(d.word_at(9), Some(6..11));
        assert_eq!(d.word_at(10), Some(6..11));
        // Between two non-word characters there is nothing to look up.
        assert_eq!(d.word_at(12), None);
        // Asking moves nothing.
        assert_eq!(d.selection(), None);
        assert_eq!(d.caret, 0);
    }

    #[test]
    fn word_at_leaves_hidden_delimiters_out_of_the_word() {
        let body = "a _important_ un**believ**able x\n";
        let d = wysiwyg_doc("word_at_hidden", body);
        let at = |needle: &str| body.find(needle).unwrap();
        let text = |r: Range<usize>| body[r].replace(['*', '_'], "");
        // Inside the emphasis, on its hidden opening `_`, and on the boundary
        // after its hidden closing one: the word is the drawn `important`.
        let important = at("important")..at("important") + "important".len();
        assert_eq!(d.word_at(at("port")), Some(important.clone()));
        assert_eq!(d.word_at(at("_important")), Some(important.clone()));
        assert_eq!(d.word_at(at("_ un")), Some(important.clone()));
        // A word the markup splits is still one word on screen.
        let w = d.word_at(at("believ")).unwrap();
        assert_eq!((w.start, text(w)), (at("un**"), "unbelievable".to_string()));
        // The source view reads the source, where `_` is a word character.
        let s = doc_with("word_at_hidden_src", body);
        assert_eq!(s.word_at(at("port")), Some(at("_important")..at(" un")));
    }

    #[test]
    fn word_helpers_respect_utf8_boundaries() {
        // "café" is 5 bytes ('é' is two); motion must land on char boundaries.
        assert_eq!(
            golden("utf8", "|café ok", |d| d.move_word_right(false)),
            "café| ok"
        );
        assert_eq!(golden("utf8b", "café |ok", |d| d.delete_word_back()), "|ok");
    }

    #[test]
    fn typing_inserts_at_the_caret_and_advances_it() {
        let mut d = doc_with("type", "hello\n");
        d.insert("Hi ");
        assert_eq!(d.source, "Hi hello\n");
        assert_eq!(d.caret, 3);
        assert!(d.dirty);
    }

    #[test]
    fn backspace_deletes_the_char_before_the_caret() {
        let mut d = doc_with("bs", "hello\n");
        d.caret = 3; // after "hel"
        d.backspace();
        assert_eq!(d.source, "helo\n");
        assert_eq!(d.caret, 2);
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut d = doc_with("replace", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6; // "word" selected
        d.insert("X");
        assert_eq!(d.source, "a X b\n");
        assert_eq!(d.caret, 3);
        assert_eq!(d.anchor, None);
    }

    #[test]
    fn toggle_bold_wraps_then_unwraps_the_selection() {
        let mut d = doc_with("bold", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
        // The toggled region stays selected, so a second toggle reverses it.
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a word b\n");
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
    }

    #[test]
    fn toggle_code_wraps_then_unwraps_the_selection() {
        let mut d = doc_with("code_rt", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Verbatim);
        assert_eq!(d.source, "a `word` b\n");
        d.toggle(InlineKind::Verbatim);
        assert_eq!(d.source, "a word b\n");
    }

    #[test]
    fn sticky_bold_with_no_selection_wraps_the_next_typed_text() {
        // ⌘b at a bare caret, then type: the text comes out bold with no
        // selection ever made — the word-processor "start bold here" gesture.
        let mut d = doc_with("sticky_wrap", "xy\n");
        d.caret = 1; // between x and y
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "xy\n", "arming a mark must not edit the document");
        d.insert("A");
        assert_eq!(d.source, "x**A**y\n");
    }

    #[test]
    fn sticky_bold_lights_the_toolbar_before_any_typing() {
        // The button must light the instant ⌘b is pressed, or the mode is
        // invisible until the first character lands.
        let mut d = doc_with("sticky_light", "xy\n");
        d.caret = 1;
        assert!(!d.active_inline_marks().contains(InlineKind::Strong));
        d.toggle(InlineKind::Strong);
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
    }

    #[test]
    fn sticky_bold_toggled_off_types_normally_again() {
        // ⌘b, type, ⌘b, type: the first run is bold, the second is not — all
        // in the flow of typing, the exact sequence the user described.
        let mut d = doc_with("sticky_off", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        d.insert("a");
        d.insert("b"); // continues inside the run, no re-arming
        assert_eq!(d.source, "**ab**\n");
        d.toggle(InlineKind::Strong); // ⌘b again — shed bold
        d.insert("c");
        assert_eq!(d.source, "**ab**c\n");
    }

    #[test]
    fn continued_typing_after_a_sticky_run_stays_in_the_run() {
        // Once a mark is realised the caret sits inside the run, so plain typing
        // extends it rather than starting a second, adjacent bold span.
        let mut d = doc_with("sticky_cont", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Emph);
        d.insert("h");
        d.insert("i");
        assert_eq!(d.source, "*hi*\n");
    }

    #[test]
    fn moving_the_caret_disarms_a_sticky_mark() {
        // Arming a mark and then moving away must not style text elsewhere.
        let mut d = doc_with("sticky_disarm", "xy\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        d.move_right(false); // caret 0 → 1, disarms
        assert!(!d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("A");
        assert_eq!(d.source, "xAy\n", "the mark must not follow the caret");
    }

    #[test]
    fn stacked_sticky_marks_apply_together() {
        // ⌘b then ⌘i before typing: the text comes out both bold and italic.
        let mut d = doc_with("sticky_stack", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        d.toggle(InlineKind::Emph);
        d.insert("x");
        // Land the caret on the styled character and confirm both marks are live.
        d.anchor = Some(d.source.find('x').unwrap());
        d.caret = d.anchor.unwrap() + 1;
        let marks = d.active_inline_marks();
        assert!(marks.contains(InlineKind::Strong), "bold: {}", d.source);
        assert!(marks.contains(InlineKind::Emph), "italic: {}", d.source);
    }

    // ── the mark-edge rule (see `Doc::splice`) ───────────────────────────────

    #[test]
    fn a_space_typed_in_a_bold_run_never_leaves_the_delimiters_showing() {
        // The reported bug, keystroke for keystroke: ⌘b, "bold", space, "hey".
        // The space inside the run made `**bold **`, which is *not* bold — four
        // literal asterisks — so the rich view drew them, correctly and
        // uselessly, until the next character happened to close the run again.
        let mut d = wysiwyg_doc("edge_typing", "a \n");
        d.caret = 2;
        d.toggle(InlineKind::Strong);
        for c in "bold".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(d.source, "a **bold**\n");
        d.insert(" ");
        assert_eq!(
            d.source, "a **bold** \n",
            "the space belongs outside the run"
        );
        assert!(
            d.active_inline_marks().contains(InlineKind::Strong),
            "bold is still what's being typed, so the button stays lit"
        );
        // What the writer is looking at while all this happens: their words.
        d.build_visual(80);
        let drawn: String = d.vmap.rows[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(drawn, "a bold ", "no delimiter ever surfaces: {}", d.source);
        for c in "hey".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(
            d.source, "a **bold hey**\n",
            "one bold phrase, not two runs"
        );
    }

    #[test]
    fn typing_past_a_space_can_still_leave_the_bold_behind() {
        // The other half: the marks stay armed across the space, so ⌘b turns
        // them off again there and the next word is plain — the run isn't
        // rejoined by a caret that was told not to.
        let mut d = wysiwyg_doc("edge_shed", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        for c in "bold ".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(d.source, "**bold** \n");
        d.toggle(InlineKind::Strong);
        assert!(!d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("x");
        assert_eq!(d.source, "**bold** x\n");
    }

    #[test]
    fn a_space_typed_first_of_all_still_leaves_the_mark_armed() {
        // ⌘b and then a space before any word: the space is not marked (nothing
        // is), and the word after it is.
        let mut d = wysiwyg_doc("edge_space_first", "a\n");
        d.caret = 1;
        d.toggle(InlineKind::Strong);
        d.insert(" ");
        assert_eq!(d.source, "a \n");
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("b");
        assert_eq!(d.source, "a **b**\n");
    }

    #[test]
    fn a_space_typed_at_either_edge_of_an_existing_mark_steps_outside_it() {
        let mut d = wysiwyg_doc("edge_tail", "x **bold**\n");
        d.caret = 8; // the caret's home at the end of the run's text
        d.insert(" ");
        assert_eq!(
            d.source, "x **bold** \n",
            "the space lands past the delimiters"
        );
        assert_eq!(d.caret, 11, "and the caret stands past it, outside the run");

        let mut d = wysiwyg_doc("edge_head", "x **bold** y\n");
        d.caret = 4; // in front of the "b"
        d.insert(" ");
        assert_eq!(d.source, "x  **bold** y\n");
        assert_eq!(d.caret, 3, "in front of the run, where the space was typed");
    }

    #[test]
    fn a_delete_that_backs_a_space_onto_a_delimiter_moves_the_delimiter() {
        // Backspace over the last letter of a bold phrase.
        let mut d = wysiwyg_doc("edge_bksp", "a **bold h**\n");
        d.caret = 10; // past the "h"
        d.backspace();
        assert_eq!(d.source, "a **bold** \n");
        assert_eq!(d.caret, 11, "the caret keeps the place on screen it had");
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("x");
        assert_eq!(d.source, "a **bold x**\n", "and typing rejoins the run");
    }

    #[test]
    fn deleting_the_last_of_a_run_takes_its_delimiters_with_it() {
        // `**b**` with the `b` gone is `****`: two delimiters with nothing to
        // mark, which is only text. The marks live on in the caret instead.
        let mut d = wysiwyg_doc("edge_empty", "a **b** c\n");
        d.caret = 5;
        d.backspace();
        assert_eq!(d.source, "a  c\n");
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.insert("x");
        assert_eq!(d.source, "a **x** c\n");
    }

    #[test]
    fn typing_over_a_whole_bold_word_keeps_it_bold() {
        let mut d = wysiwyg_doc("edge_replace", "a **bold** c\n");
        d.anchor = Some(4);
        d.caret = 8; // the word, not its delimiters
        d.insert("x");
        assert_eq!(d.source, "a **x** c\n");
    }

    #[test]
    fn a_code_span_keeps_the_space_it_is_given() {
        // Backticks are not whitespace-sensitive the way `**` is: `` `code ` ``
        // is still verbatim, so nothing is re-spelt. The repair asks the parser
        // rather than a table of kinds, and this is the answer it gets.
        let mut d = wysiwyg_doc("edge_code", "a `code` c\n");
        d.caret = 7;
        d.insert(" ");
        assert_eq!(d.source, "a `code ` c\n");
    }

    #[test]
    fn a_delete_from_a_runs_outer_edge_reaches_into_the_run() {
        // A run's closing delimiter has a caret home on each side of it, one
        // column apart on screen — and a plain ← off the space after a bold word
        // lands on the outer one. The character drawn behind the caret there is
        // still the last letter of the phrase, so that is what Backspace takes;
        // the byte behind it is a `*` nobody can see.
        let mut d = wysiwyg_doc("edge_outer_close", "**bold** x\n");
        d.caret = 9;
        d.move_left(false);
        assert_eq!(d.caret, 8, "← rests past the delimiters, not inside them");
        d.backspace();
        assert_eq!(
            d.source, "**bol** x\n",
            "a letter of the phrase, not its `*`"
        );
        assert_eq!(d.caret, 5);

        // And the mirror in front of the opening delimiter, where Delete's
        // character is the first letter of the run.
        let mut d = wysiwyg_doc("edge_outer_open", "x**bold**\n");
        d.caret = 1;
        d.delete_forward();
        assert_eq!(d.source, "x**old**\n");
        assert_eq!(d.caret, 3, "inside the run, in front of what is left of it");
    }

    #[test]
    fn a_delete_at_a_run_edge_never_eats_a_delimiter() {
        // The byte beside the caret at either edge of a bold word is a `*` the
        // rich view draws nothing for. Taking it is not the character delete the
        // key was pressed for — it unspells the run and puts a literal asterisk
        // on screen (`a *bold** c`). The visible character is the one that goes.
        let mut d = wysiwyg_doc("edge_open_bksp", "a **bold** c\n");
        d.caret = 4; // in front of the "b"
        d.backspace();
        assert_eq!(d.source, "a**bold** c\n", "the space goes, the run stands");

        let mut d = wysiwyg_doc("edge_close_del", "a **bold** c\n");
        d.caret = 8; // past the "d"
        d.delete_forward();
        assert_eq!(d.source, "a **bold**c\n");
        assert_eq!(d.caret, 8, "and the caret stays inside the run");
        d.insert("x");
        assert_eq!(d.source, "a **boldx**c\n");

        // A code span's backticks are hidden the same way, so they are covered
        // by the same rule and not by a list of kinds.
        let mut d = wysiwyg_doc("edge_open_code", "a `code` c\n");
        d.caret = 3;
        d.backspace();
        assert_eq!(d.source, "a`code` c\n");
    }

    #[test]
    fn the_source_view_deletes_the_delimiter_byte_it_is_shown() {
        // The asterisks are on the screen there and the caret can stand between
        // them, so a delete takes exactly the byte it is aimed at.
        let mut d = doc_with("edge_open_src", "a **bold** c\n");
        d.caret = 4;
        d.backspace();
        assert_eq!(d.source, "a *bold** c\n");

        let mut d = doc_with("edge_close_src", "a **bold** c\n");
        d.caret = 8;
        d.delete_forward();
        assert_eq!(d.source, "a **bold* c\n");
    }

    #[test]
    fn backspacing_the_space_out_of_a_bold_phrase_leaves_the_caret_in_it() {
        // The reported bug, keystroke for keystroke: ⌘b, "bold", space, Backspace.
        // The space had stepped outside the run (the mark-edge rule), taking the
        // caret with it, so the delete put it back down on the far side of the
        // closing `**` — one place on screen, and the wrong side of it. Typing
        // came out plain and the toolbar went dark, with nothing to see.
        let mut d = wysiwyg_doc("edge_bksp_space", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        for c in "bold".chars() {
            d.insert(&c.to_string());
        }
        d.insert(" ");
        assert_eq!(d.source, "**bold** \n");
        d.backspace();
        assert_eq!(
            d.source, "**bold**\n",
            "the space goes, the delimiters stay"
        );
        assert_eq!(d.caret, 6, "and the caret comes back inside the run");
        assert!(
            d.active_inline_marks().contains(InlineKind::Strong),
            "so the button is still lit"
        );
        d.insert("x");
        assert_eq!(
            d.source, "**boldx**\n",
            "and the next character is still bold"
        );
    }

    #[test]
    fn a_second_backspace_there_deletes_a_letter_of_the_phrase() {
        // What the stranded caret did next: the byte behind it was the closing
        // `*`, so a second press took that instead of a letter — `**bold*`, the
        // styling gone and an asterisk on the screen where the word had been.
        let mut d = wysiwyg_doc("edge_bksp_twice", "\n");
        d.caret = 0;
        d.toggle(InlineKind::Strong);
        for c in "bold ".chars() {
            d.insert(&c.to_string());
        }
        assert_eq!(d.source, "**bold** \n");
        d.backspace();
        d.backspace();
        assert_eq!(d.source, "**bol**\n", "the delete lands inside the run");
        assert_eq!(d.caret, 5);
    }

    #[test]
    fn a_delete_that_ends_at_a_nested_run_settles_inside_every_delimiter() {
        // `***both***` closes two runs with one stack of asterisks: the caret has
        // to walk in through all of them, or it lands between the emph and the
        // strong and types half-marked.
        let mut d = wysiwyg_doc("edge_bksp_nested", "***both*** \n");
        d.caret = 11;
        d.backspace();
        assert_eq!(d.source, "***both***\n");
        assert_eq!(d.caret, 7, "past the last letter, inside both runs");
        d.insert("x");
        assert_eq!(d.source, "***bothx***\n");
    }

    #[test]
    fn a_delete_that_ends_mid_run_leaves_the_caret_where_it_fell() {
        // The settle only moves a caret a run actually closed over. Ordinary
        // deletes — inside a run, or in plain prose — are untouched.
        let mut d = wysiwyg_doc("edge_bksp_mid", "a **bold** c\n");
        d.caret = 8;
        d.backspace();
        assert_eq!(d.source, "a **bol** c\n");
        assert_eq!(d.caret, 7);

        let mut d = wysiwyg_doc("edge_bksp_plain", "plain\n");
        d.caret = 5;
        d.backspace();
        assert_eq!(d.source, "plai\n");
        assert_eq!(d.caret, 4);
    }

    #[test]
    fn the_source_view_leaves_a_delete_where_it_landed() {
        // The delimiters are on the screen there, so the offset past them is a
        // place the caret can be seen to be — nothing to settle.
        let mut d = doc_with("edge_bksp_src", "**bold** \n");
        d.caret = 9;
        d.backspace();
        assert_eq!(d.source, "**bold**\n");
        assert_eq!(d.caret, 8);
    }

    #[test]
    fn the_mark_edge_rule_clears_every_delimiter_of_a_nested_run() {
        // `***both***` closes two runs with one stack of asterisks; a space that
        // clears only the inner one lands against the outer's and breaks that
        // instead.
        let mut d = wysiwyg_doc("edge_nested", "a ***both***\n");
        d.caret = 9;
        d.insert(" ");
        assert_eq!(d.source, "a ***both*** \n");
        assert_eq!(d.caret, 13);
        d.insert("x");
        assert_eq!(d.source, "a ***both x***\n");
    }

    #[test]
    fn the_mark_edge_repair_undoes_with_the_keystroke_that_caused_it() {
        // The delimiter shuffle is not an edit the writer made, so it is not a
        // step they have to undo past.
        let mut d = wysiwyg_doc("edge_undo", "a **bold**\n");
        d.caret = 8;
        d.insert(" ");
        assert_eq!(d.source, "a **bold** \n");
        d.undo();
        assert_eq!(d.source, "a **bold**\n");
    }

    #[test]
    fn the_source_view_types_the_space_where_it_was_asked_to() {
        // The rule is a rich-view courtesy. In the source view the delimiters are
        // on the screen and the user is editing the bytes they can see.
        let mut d = doc_with("edge_src", "a **bold** c\n");
        d.caret = 8;
        d.insert(" ");
        assert_eq!(d.source, "a **bold ** c\n");
    }

    #[test]
    fn toggling_a_mark_over_a_selection_leaves_its_edge_whitespace_out() {
        // Double-clicking a word takes the space after it; bolding that must not
        // spell `**word **`, which is not bold at all.
        let mut d = wysiwyg_doc("edge_sel", "a word b\n");
        d.anchor = Some(2);
        d.caret = 7; // "word "
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a word b\n");
        d.toggle(InlineKind::Strong);
        assert_eq!(
            d.source, "a **word** b\n",
            "reapplying the mark must not wrap stale delimiter offsets"
        );
        // And a selection of nothing but whitespace has no word to mark.
        let mut d = wysiwyg_doc("edge_sel_ws", "a word b\n");
        d.anchor = Some(6);
        d.caret = 7;
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a word b\n");
        assert!(d.status.is_some());
    }

    #[test]
    fn set_block_turns_a_paragraph_into_a_heading_at_the_caret() {
        let mut d = doc_with("head_set", "hello\n");
        d.caret = 2; // caret inside the paragraph, no selection
        d.set_block(BlockKind::Heading(1));
        assert_eq!(d.source, "# hello\n");
    }

    #[test]
    fn set_block_heading_works_in_wysiwyg_view() {
        // The app defaults to WYSIWYG; the caret is a source offset either way.
        let mut d = wysiwyg_doc("head_wys", "hello\n");
        d.caret = 2;
        d.set_block(BlockKind::Heading(1));
        assert_eq!(d.source, "# hello\n");
    }

    #[test]
    fn toggle_heading_applies_switches_and_reverts() {
        let mut d = doc_with("head_toggle", "hello\n");
        d.caret = 2;
        d.toggle_heading(1);
        assert_eq!(d.source, "# hello\n"); // paragraph → H1
        d.toggle_heading(2);
        assert_eq!(d.source, "## hello\n"); // H1 → H2 (different level switches)
        d.toggle_heading(2);
        assert_eq!(d.source, "hello\n"); // same level reverts to paragraph
    }

    #[test]
    fn preserve_enter_at_a_line_end_lands_the_caret_on_the_new_blank_line() {
        // Regression: Enter at the end of a soft-break line (mid-paragraph) opened
        // the blank line but the caret rendered on the *next* line, because the
        // separator was a non-navigable decoration row. In Preserve flow that
        // blank line is a real caret home — the caret must resolve onto it, and
        // typing there makes the soft break that continues the paragraph.
        let src = "line one:\nsecond line\n";
        let mut d = wysiwyg_doc("pre_enter_lineend", src);
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual_unwrapped(); // the GUI path (pixel-wrapped)
        d.caret = 9; // the visual end of row 0, at the soft-break '\n'
        d.newline();
        d.build_visual_unwrapped();
        assert_eq!(d.source, "line one:\n\nsecond line\n");
        assert_eq!(
            d.caret, 10,
            "caret sits on the new blank line, not the next line"
        );
        // The blank line is row 1, and the caret resolves onto it — not row 2.
        assert_eq!(
            d.vmap.pos_of_offset(10),
            (1, 0),
            "caret renders on the blank row"
        );
        assert!(
            !d.vmap.rows[1].decoration,
            "the blank line is navigable in Preserve"
        );
        // Typing there makes a soft break: one paragraph, three lines.
        d.insert("new clause,");
        assert_eq!(d.source, "line one:\nnew clause,\nsecond line\n");
    }

    #[test]
    fn preserve_enter_makes_a_soft_break_not_a_paragraph() {
        // Mid-paragraph: Enter splits the line with a single `\n`, a soft break
        // that keeps it one paragraph — where Fold would open a second paragraph.
        let mut d = wysiwyg_doc("pre_enter_mid", "abcdef\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 3;
        d.newline();
        assert_eq!(d.source, "abc\ndef\n", "mid-line Enter is a soft break");

        // End-of-paragraph: Enter then typing continues the same paragraph on a
        // new line (a soft break), not a fresh paragraph.
        let mut d = wysiwyg_doc("pre_enter_end", "abc\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 3;
        d.newline();
        d.insert("def");
        assert_eq!(
            d.source, "abc\ndef\n",
            "end-of-line Enter + typing is a soft break"
        );
    }

    #[test]
    fn preserve_double_enter_still_makes_a_paragraph() {
        // Two Enters in a row promote to a real paragraph break: the second lands
        // on the blank line the first opened and takes the empty-line branch.
        let mut d = wysiwyg_doc("pre_enter_dbl", "abc\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 3;
        d.newline();
        d.newline();
        d.insert("def");
        assert_eq!(
            d.source, "abc\n\ndef\n",
            "double Enter is a paragraph break"
        );
    }

    #[test]
    fn preserve_backspace_joins_across_a_soft_break() {
        // Backspace is the symmetric undo of a Preserve Enter: over the `\n` of a
        // soft break it deletes the single newline and joins the two lines.
        let mut d = wysiwyg_doc("pre_bs", "abc\ndef\n");
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
        d.caret = 4; // start of "def", just past the soft break
        d.backspace();
        assert_eq!(
            d.source, "abcdef\n",
            "Backspace joins across the soft break"
        );
        assert_eq!(d.caret, 3, "caret lands where the lines meet");
    }

    #[test]
    fn fold_enter_still_starts_a_new_paragraph() {
        // The default flow is unchanged: a lone `\n` would render as an invisible
        // space, so Enter keeps opening the paragraph break that actually shows.
        let mut d = wysiwyg_doc("fold_enter", "abcdef\n");
        d.caret = 3;
        d.newline();
        assert_eq!(
            d.source, "abc\n\ndef\n",
            "Fold mid-line Enter is a paragraph break"
        );
    }

    #[test]
    fn wysiwyg_one_enter_starts_a_new_paragraph() {
        // Regression: one Enter left the caret between the two newlines, so typing
        // made a soft break (one paragraph) and you needed a second Enter.
        let mut d = wysiwyg_doc("wys_enter", "abc\n");
        d.caret = 3;
        d.newline();
        d.insert("def");
        assert_eq!(d.source, "abc\n\ndef\n"); // two paragraphs, not "abc\ndef\n"
    }

    #[test]
    fn enter_at_the_end_of_a_bold_run_keeps_its_closing_delimiter_attached() {
        // Regression: Enter at the caret's natural End-of-line resting place
        // after a bold run with nothing following it (on screen: right after
        // "bold", before the hidden closing "**") spliced the paragraph break
        // at that very byte offset — which sits *before* the closing "**" in
        // the source, since the delimiter is hidden and emits no glyph of its
        // own for `push_row`'s "end of row" fallback to count. That severed the
        // mark: "**bold**\n" became "**bold\n\n**\n", stranding the closing
        // "**" alone on the new line instead of leaving "**bold**" intact with
        // a fresh empty paragraph after it.
        let mut d = wysiwyg_doc("bold_eol_enter", "**bold**\n");
        d.move_end(false); // the WYSIWYG End key, from caret 0
        assert_eq!(
            d.caret, 6,
            "caret rests right after \"bold\", before the hidden \"**\""
        );
        d.newline();
        assert!(
            d.source.starts_with("**bold**"),
            "the closing ** must stay attached to \"bold\": got {:?}",
            d.source
        );
        assert_eq!(
            d.source, "**bold**\n\n\n",
            "a fresh empty paragraph follows the still-intact bold run"
        );
    }

    #[test]
    fn source_view_enter_is_a_single_newline() {
        let mut d = doc_with("src_enter", "abc\n");
        d.caret = 3;
        d.newline();
        assert_eq!(d.source, "abc\n\n");
    }

    #[test]
    fn heading_applies_at_the_end_of_a_paragraph() {
        // The caret at a line end sits at the doc level; set_block must still find
        // the block on that line.
        let mut d = doc_with("head_end", "abc\n");
        d.caret = 3; // end of "abc"
        d.toggle_heading(1);
        assert_eq!(d.source, "# abc\n");
    }

    #[test]
    fn heading_on_an_empty_new_paragraph_creates_one() {
        let mut d = wysiwyg_doc("head_empty", "abc\n");
        d.caret = 3;
        d.newline(); // caret now on a fresh, empty paragraph
        d.toggle_heading(1);
        d.insert("Title");
        assert!(d.source.contains("# Title"), "got {:?}", d.source);
    }

    #[test]
    fn a_heading_typed_on_a_blank_line_keeps_the_caret_on_its_own_row() {
        // The reported bug, end to end: click a blank line with another one under
        // it, press H1, type. The text landed in the heading and the caret's
        // offset was right (the source view drew it there), but the rich view
        // drew it two rows lower, on the trailing blank line — the empty `# `
        // heading had left every row below it short by the marker's two bytes,
        // and the blank line ended up claiming the heading's own end offset.
        let mut d = wysiwyg_doc("head_blank", "one\n\ntwo\n\n\n\n");
        d.build_visual_unwrapped();
        d.caret = d.vmap.offset_of_pos(4, 0); // the first of the two blank lines
        d.toggle_heading(1);
        for c in "title".chars() {
            d.insert(&c.to_string());
            d.build_visual_unwrapped(); // as a frontend does, one frame per key
        }
        assert_eq!(d.source, "one\n\ntwo\n\n# title\n\n");
        assert_eq!(
            d.caret_pos(),
            (4, 5),
            "the caret draws at the end of the heading"
        );
    }

    #[test]
    fn clicking_an_empty_heading_types_after_its_marker() {
        // The same anchor from the other side: the empty heading's row is its own
        // caret home, so a click on it must land past the hidden `# `. Landing in
        // front of the hashes made the first keystroke un-heading the line.
        let mut d = wysiwyg_doc("head_click", "# \n");
        d.build_visual_unwrapped();
        d.caret = d.vmap.offset_of_pos(0, 0);
        d.insert("x");
        assert_eq!(d.source, "# x\n");
    }

    #[test]
    fn wysiwyg_enter_after_a_heading_makes_a_paragraph() {
        let mut d = wysiwyg_doc("head_enter", "# Title\n");
        d.caret = 7; // end of the heading
        d.newline();
        d.insert("body");
        assert_eq!(d.source, "# Title\n\nbody\n");
    }

    #[test]
    fn wysiwyg_enter_continues_a_bullet_list() {
        let mut d = wysiwyg_doc("wys_bullet", "- item\n");
        d.caret = 6; // end of "item"
        d.newline();
        d.insert("two");
        assert_eq!(d.source, "- item\n- two\n");
    }

    #[test]
    fn wysiwyg_enter_increments_an_ordered_list() {
        let mut d = wysiwyg_doc("wys_ol", "1. one\n");
        d.caret = 6; // end of "one"
        d.newline();
        d.insert("two");
        assert_eq!(d.source, "1. one\n2. two\n");
    }

    #[test]
    fn wysiwyg_backspace_after_leaving_a_list_collapses_the_gap_cleanly() {
        // Regression for the "extra newline" left between a list and the paragraph
        // below it. Enter, Enter leaves the list on a fresh empty paragraph
        // (`- item\n\n\n\nnext`, a navigable blank between the two blocks); one
        // Backspace should then take the caret cleanly back to the end of the list
        // item, `- item\n\nnext`, not delete a single newline and strand it on the
        // odd `- item\n\n\nnext` — a blank line the eye reads as one separator but
        // no caret can land on. The map is rebuilt between keystrokes exactly as a
        // frontend does, since Backspace reads the stop table to place the delete.
        let mut d = wysiwyg_doc("wys_exit_bksp", "- item\n\nnext\n");
        d.caret = 6; // end of "item"
        d.newline();
        d.build_visual(80);
        d.newline(); // leave the list onto a fresh empty paragraph
        d.build_visual(80);
        assert_eq!(
            d.source, "- item\n\n\n\nnext\n",
            "double-Enter opens the empty paragraph"
        );
        d.backspace();
        assert_eq!(
            d.source, "- item\n\nnext\n",
            "one Backspace collapses the whole gap"
        );
        assert_eq!(
            d.caret, 6,
            "and lands the caret back at the end of the list item"
        );
    }

    #[test]
    fn wysiwyg_backspace_on_stacked_blank_lines_still_removes_just_one() {
        // The stop-wise delete must not over-reach when there is no block boundary
        // to cross: two blank lines in a row are one caret stop apart, so pressing
        // Enter on an empty line and then Backspace removes exactly the one newline
        // it added — the lone-Enter / lone-Backspace symmetry, preserved.
        let mut d = wysiwyg_doc("wys_stack", "abc\n\n\n");
        d.caret = 5; // the empty paragraph the first Enter already opened
        d.build_visual(80);
        d.newline();
        d.build_visual(80);
        assert_eq!(
            d.source, "abc\n\n\n\n",
            "Enter on the blank line adds one newline"
        );
        d.backspace();
        assert_eq!(
            d.source, "abc\n\n\n",
            "Backspace takes back exactly that one newline"
        );
    }

    #[test]
    fn wysiwyg_enter_on_an_empty_list_item_exits_the_list() {
        let mut d = wysiwyg_doc("wys_exit", "- a\n- \n");
        d.caret = 6; // end of the empty "- " item
        d.newline();
        d.insert("p");
        assert_eq!(d.source, "- a\n\np\n");
    }

    #[test]
    fn wysiwyg_enter_does_not_mistake_a_setext_underline_for_a_list() {
        // `text\n- \n` is a setext heading — the `- ` is its underline, not a
        // list item, though it reads as a `- ` marker byte-for-byte. Enter must
        // not take the list-exit path (which would splice the `- ` away as if
        // leaving an empty item); the AST guard sends it to a normal break and
        // leaves the underline intact.
        let mut d = wysiwyg_doc("wys_setext", "text\n- \n");
        assert!(
            d.nodes().iter().any(|n| n.kind == Kind::Heading),
            "precondition: twig parses this as a heading, not a list",
        );
        d.caret = 7; // on the `- ` underline line
        d.newline();
        assert!(
            d.source.contains("- "),
            "the setext underline survives, not spliced away as a list item: {:?}",
            d.source,
        );
    }

    #[test]
    fn wysiwyg_enter_in_a_code_block_is_a_literal_newline() {
        let mut d = wysiwyg_doc("wys_code", "```\nabc\n```\n");
        d.caret = 7; // end of "abc" inside the fence
        d.newline();
        d.insert("def");
        assert_eq!(d.source, "```\nabc\ndef\n```\n");
    }

    #[test]
    fn wysiwyg_enter_continues_a_block_quote() {
        // Enter opens a new *paragraph* inside the quote, not a second line of
        // the same one. `> quote\n> more` is a soft break, which under
        // `LineFlow::Fold` renders as a space — the keystroke would look like it
        // did nothing. The quoted blank line is what makes the break visible, and
        // it's the same thing Enter does in running prose.
        let mut d = wysiwyg_doc("wys_quote", "> quote\n");
        d.caret = 7; // end of "quote"
        d.newline();
        d.insert("more");
        assert_eq!(d.source, "> quote\n>\n> more\n");
        // Still one quote, now holding two paragraphs — not a quote and a stray
        // line that fell out of it.
        let quotes = d
            .nodes()
            .iter()
            .filter(|n| n.kind == Kind::BlockQuote)
            .count();
        assert_eq!(quotes, 1);
    }

    #[test]
    fn set_block_makes_a_heading_at_the_caret() {
        let mut d = doc_with("head", "Title\n\nbody\n");
        d.caret = 0;
        d.set_block(BlockKind::Heading(2));
        assert_eq!(d.source, "## Title\n\nbody\n");
        d.set_block(BlockKind::Paragraph);
        assert_eq!(d.source, "Title\n\nbody\n");
    }

    // ── block containers (quote / list) ──────────────────────────────────────

    #[test]
    fn toggle_blockquote_wraps_the_block_at_the_caret_and_reverses() {
        let g = |m, f: fn(&mut Doc)| golden("quote", m, f);
        assert_eq!(g("hel|lo\n", |d| d.toggle_blockquote()), "> hel|lo\n");
        assert_eq!(g("> hel|lo\n", |d| d.toggle_blockquote()), "hel|lo\n");
        // A caret at a line end sits at the doc level; the block is still found.
        assert_eq!(g("hello|\n", |d| d.toggle_blockquote()), "> hello|\n");
    }

    #[test]
    fn toggle_blockquote_keeps_the_caret_in_a_hard_wrapped_paragraph() {
        // Every source line of the paragraph gets its own `> `, so a caret left
        // on its old byte offset falls one prefix per line above it too far
        // back — inside the markup it just asked for rather than in its word.
        assert_eq!(
            golden("quote_wrap", "aaa\nb|bb\nccc\n", |d| d.toggle_blockquote()),
            "> aaa\n> b|bb\n> ccc\n"
        );
    }

    #[test]
    fn toggle_blockquote_works_in_wysiwyg_view() {
        let g = |n, m, f: fn(&mut Doc)| golden_in(View::Wysiwyg, n, m, f);
        assert_eq!(
            g("q_wys", "hel|lo\n", |d| d.toggle_blockquote()),
            "> hel|lo\n"
        );
        assert_eq!(
            g("q_wys2", "> hel|lo\n", |d| d.toggle_blockquote()),
            "hel|lo\n"
        );
    }

    #[test]
    fn toggle_list_makes_a_list_and_converts_between_the_kinds() {
        let g = |m, f: fn(&mut Doc)| golden("list", m, f);
        assert_eq!(g("hel|lo\n", |d| d.toggle_list(false)), "- hel|lo\n");
        assert_eq!(g("hel|lo\n", |d| d.toggle_list(true)), "1. hel|lo\n");
        // The *other* kind converts in place instead of nesting, which is what
        // makes the two buttons one three-state control.
        assert_eq!(g("- hel|lo\n", |d| d.toggle_list(true)), "1. hel|lo\n");
        assert_eq!(g("1. hel|lo\n", |d| d.toggle_list(false)), "- hel|lo\n");
        // Its own kind, over the only item the list holds, takes it off.
        assert_eq!(g("- hel|lo\n", |d| d.toggle_list(false)), "hel|lo\n");
    }

    #[test]
    fn toggle_list_works_in_wysiwyg_view() {
        let g = |n, m, f: fn(&mut Doc)| golden_in(View::Wysiwyg, n, m, f);
        assert_eq!(
            g("l_wys", "hel|lo\n", |d| d.toggle_list(true)),
            "1. hel|lo\n"
        );
        assert_eq!(
            g("l_wys2", "1. hel|lo\n", |d| d.toggle_list(false)),
            "- hel|lo\n"
        );
        assert_eq!(
            g("l_wys3", "- hel|lo\n", |d| d.toggle_list(false)),
            "hel|lo\n"
        );
    }

    #[test]
    fn a_list_over_a_selection_numbers_each_block_and_stays_selected() {
        // The selection has to grow with the markup: twig takes a container off
        // only a range covering every block it holds, so the second press can
        // reverse the first only if the result is what's selected.
        let mut d = doc_with("list_sel", "abc\n\ndef\n");
        d.select_all();
        d.toggle_list(true);
        assert_eq!(d.source, "1. abc\n\n2. def\n");
        assert_eq!(d.selection(), Some((0, d.source.len())));
        d.toggle_list(true);
        assert_eq!(d.source, "abc\n\ndef\n");
    }

    #[test]
    fn toggle_blockquote_nests_a_partly_covered_quote() {
        // twig's rule: covering only some of a container's blocks nests, because
        // taking the quote off would drag its uncovered siblings out with it.
        let mut d = doc_with("quote_nest", "> a\n>\n> b\n");
        d.caret = 2; // in the first quoted paragraph only
        d.toggle_blockquote();
        assert_eq!(d.source, "> > a\n>\n> b\n");
    }

    #[test]
    fn a_container_toggle_opens_an_empty_one_on_a_blank_line() {
        // A blank line used to be no block for twig to wrap —
        // `toggle_block_container` answered `NotFound` — so Quote and the list
        // buttons did nothing on the very line the H1 button works on, and leaf
        // lent twig a scratch paragraph to wrap and took it back out again.
        // twig 3.2.0 opens an empty container there itself, so what is left here
        // is where the caret lands: inside the marker that was just written.
        let mut d = doc_with("quote_blank", "\nabc\n");
        d.caret = 0;
        d.toggle_blockquote();
        assert_eq!(d.source, "> \nabc\n");
        assert_eq!(
            d.caret, 2,
            "the caret belongs inside the quote it just opened"
        );
        assert!(d.status.is_none(), "{:?}", d.status);
        assert!(d.dirty);

        // And the paragraph below is still its own block: an empty container one
        // soft break from `abc` would take that paragraph into the quote with it.
        let mut d = wysiwyg_doc("quote_blank_rows", "\nabc\n");
        d.caret = 0;
        d.toggle_blockquote();
        d.build_visual(80);
        assert_eq!(drawn_rows(&d), ["│ ", "", "abc"]);

        // The same from the other side: a blank line directly under a paragraph
        // earns the blank line an empty block needs, rather than being read as a
        // soft break inside that paragraph.
        let mut d = doc_with("list_blank_below", "abc\n");
        d.caret = 4;
        d.toggle_list(false);
        assert_eq!(d.source, "abc\n\n- ");
        assert_eq!(d.caret, 7);
    }

    #[test]
    fn enter_at_the_end_of_a_quote_stays_in_the_quote() {
        // The gesture the rendering fix is for. `newline` inside a quote already
        // wrote the right source — `> a\n` becomes `> a\n>\n> \n`, twig's own
        // spelling — but the two marker lines it adds belonged to no node until
        // twig 3.2.0, so the gutter stopped at `a` and the line the writer had
        // just made drew as plain prose under the quote.
        let mut d = wysiwyg_doc("quote_enter", "> a\n");
        d.caret = 3; // past `a`, at the end of the quoted line
        d.newline();
        assert_eq!(d.source, "> a\n>\n> \n");
        d.build_visual(80);
        assert_eq!(drawn_rows(&d), ["│ a", "│ ", "│ "]);
        // And the caret is on the new line, not stranded on the old one.
        assert_eq!(d.caret, 8);
    }

    #[test]
    fn a_second_enter_at_the_end_of_a_quote_leaves_it() {
        // Double-Enter, as it leaves a list. The second Enter used to split the
        // empty quoted line like any other, so the writer pressing it to get
        // out watched blank lines pile up under the quote with the caret still
        // in it — and the empty quoted line and its separator stayed behind.
        let mut d = wysiwyg_doc("quote_exit", "> hello\n");
        d.caret = 7;
        d.newline();
        assert_eq!(d.source, "> hello\n>\n> \n");
        d.newline();
        assert_eq!(d.source, "> hello\n\n\n");
        d.insert("x");
        assert_eq!(d.source, "> hello\n\nx\n");
        let quotes = d
            .nodes()
            .iter()
            .filter(|n| n.kind == Kind::BlockQuote)
            .count();
        assert_eq!(quotes, 1, "the quote keeps only `hello`");

        // Out of the inner quote only, into a paragraph of the outer one.
        let mut d = wysiwyg_doc("quote_exit_nested", "> > a\n");
        d.caret = 5;
        d.newline();
        d.newline();
        assert_eq!(d.source, "> > a\n>\n> \n");

        // Above more of the document, the quote's end is still its end.
        let mut d = wysiwyg_doc("quote_exit_mid", "> a\n\nnext\n");
        d.caret = 3;
        d.newline();
        d.newline();
        d.insert("x");
        assert_eq!(d.source, "> a\n\nx\n\nnext\n");

        // And one undo puts the empty quoted line back.
        let mut d = wysiwyg_doc("quote_exit_undo", "> a\n");
        d.caret = 3;
        d.newline();
        d.newline();
        d.undo();
        assert_eq!(d.source, "> a\n>\n> \n");
    }

    #[test]
    fn the_line_a_quote_is_left_on_is_spaced_as_the_paragraph_it_becomes() {
        // Above a heading, the gap over the empty line was labelled by the
        // heading beyond it, which a frontend spaces wider: the line sat low,
        // and jumped up as soon as a character made it a paragraph.
        let boundaries = |d: &mut Doc| {
            d.build_visual(80);
            d.vmap.rows.iter().map(|r| r.boundary).collect::<Vec<_>>()
        };
        let mut d = wysiwyg_doc("quote_exit_gap", "> a\n\n## Math\n");
        d.caret = 3;
        d.newline();
        d.newline();
        let empty = boundaries(&mut d);
        d.insert("hello");
        assert_eq!(boundaries(&mut d), empty);
    }

    #[test]
    fn undoing_enter_in_a_quote_puts_the_caret_back_where_it_was() {
        // Enter in a quote or a list goes to twig's `split_block`, which used to
        // be handed no pre-edit caret. Undo restored whatever the last edit had
        // filed — here the line under the quote, a gap the view moved the caret
        // off and onto the heading below.
        for src in ["> a\n\n## Math\n", "- a\n\n## Math\n"] {
            let mut d = wysiwyg_doc("split_undo_caret", src);
            d.caret = 3;
            d.newline();
            assert_ne!(d.source, src);
            d.undo();
            assert_eq!(d.source, src);
            assert_eq!(d.caret, 3, "{src:?}");
        }
    }

    #[test]
    fn opening_a_container_on_a_blank_line_is_one_undo_step() {
        // It was three edits — scratch, wrap, unscratch — coalesced into one, and
        // now it is twig's single edit. Either way one ⌘z has to put the blank
        // line back rather than undoing into a half-built document.
        for open in [
            &(|d: &mut Doc| d.toggle_blockquote()) as &dyn Fn(&mut Doc),
            &|d: &mut Doc| d.toggle_list(false),
            &|d: &mut Doc| d.toggle_list(true),
        ] {
            let mut d = doc_with("container_blank_undo", "a\n\n\n\nb\n");
            d.caret = 3;
            open(&mut d);
            assert_ne!(d.source, "a\n\n\n\nb\n");
            d.undo();
            assert_eq!(d.source, "a\n\n\n\nb\n");
        }
    }

    #[test]
    fn a_container_toggle_is_one_undo_step() {
        let mut d = doc_with("quote_undo", "hello\n");
        d.caret = 3;
        d.insert("X"); // a typing run the structural edit must not fold into
        d.toggle_blockquote();
        assert_eq!(d.source, "> helXlo\n");
        d.undo();
        assert_eq!(d.source, "helXlo\n");
    }

    // ── links ────────────────────────────────────────────────────────────────

    #[test]
    fn insert_link_wraps_the_selection_and_leaves_its_text_selected() {
        let mut d = doc_with("link_sel", "word here\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("http://x.dev");
        assert_eq!(d.source, "[word](http://x.dev) here\n");
        // The text, not the destination — so a second press re-points the link
        // the first one made rather than nesting one inside it.
        assert_eq!(d.selected_text(), Some("word"));
        d.insert_link("http://y.dev");
        assert_eq!(d.source, "[word](http://y.dev) here\n");
        assert_eq!(d.selected_text(), Some("word"));
    }

    #[test]
    fn insert_image_at_the_caret_spells_the_markup_and_lands_past_it() {
        let mut d = doc_with("img_caret", "before after\n");
        d.caret = 7; // between "before " and "after"
        d.insert_image("cat.png", "a cat");
        assert_eq!(d.source, "before ![a cat](cat.png)after\n");
        // The caret sits just past the inserted image, nothing selected.
        assert_eq!(d.selection(), None);
        assert_eq!(d.caret, 7 + "![a cat](cat.png)".len());
    }

    /// The bug a real vault hit: a filename with spaces in it. Markdown ends a
    /// destination at the first space, so the `format!` this used to be wrote
    /// something that was not an image at all — and the reader saw the markup as
    /// text. twig owns the spelling now, and moves it into the angle form.
    #[test]
    fn insert_image_spells_a_destination_with_spaces_so_it_stays_an_image() {
        let mut d = doc_with("img_space", "x\n");
        d.caret = 0;
        d.insert_image("Jesus Commands the Apostles to Rest.jpg", "");
        assert_eq!(
            d.source,
            "![](<Jesus Commands the Apostles to Rest.jpg>)x\n"
        );
        // And it reads back as an image pointing at the unescaped path — the angle
        // brackets are spelling, not part of the destination.
        d.caret = 2;
        assert_eq!(
            d.image_destination_at_caret(),
            Some("Jesus Commands the Apostles to Rest.jpg".to_string())
        );
    }

    /// A `)` in a caption or a filename must not close the image early.
    #[test]
    fn insert_image_escapes_a_paren_in_either_half() {
        let mut d = doc_with("img_paren", "x\n");
        d.caret = 0;
        d.insert_image("a)b.png", "");
        assert_eq!(d.source, "![](a\\)b.png)x\n");
        d.caret = 2;
        assert_eq!(d.image_destination_at_caret(), Some("a)b.png".to_string()));
    }

    #[test]
    fn insert_image_uses_the_selection_as_alt_text() {
        let mut d = doc_with("img_sel", "caption here\n");
        d.anchor = Some(0);
        d.caret = 7; // "caption"
        d.insert_image("p.png", "ignored fallback");
        assert_eq!(d.source, "![caption](p.png) here\n");
    }

    #[test]
    fn insert_image_with_no_alt_leaves_empty_brackets() {
        let mut d = doc_with("img_noalt", "\n");
        d.caret = 0;
        d.insert_image("logo.svg", "");
        assert_eq!(d.source, "![](logo.svg)\n");
    }

    // ── move_block ─────────────────────────────────────────────────────────────

    /// A move, then its undo: one step takes the whole thing back.
    fn moved(name: &str, body: &str, from: usize, to: usize) -> (String, Doc) {
        let mut d = doc_with(name, body);
        d.caret = from;
        let steps = d.undo_steps;
        d.move_block(from, to);
        let after = d.source.clone();
        assert_eq!(d.undo_steps, steps + 1, "one undo step");
        assert!(d.dirty);
        d.undo();
        assert_eq!(d.source, body, "one undo restores the original");
        (after, d)
    }

    #[test]
    fn move_block_carries_a_paragraph_between_two_paragraphs_and_to_the_end() {
        let body = "a\n\nb\n\nc\n";
        assert_eq!(moved("mv_p1", body, 6, 3).0, "a\n\nc\n\nb\n");
        assert_eq!(moved("mv_p2", body, 0, body.len()).0, "b\n\nc\n\na\n");
        assert_eq!(moved("mv_p3", body, 3, 0).0, "b\n\na\n\nc\n");
    }

    #[test]
    fn move_block_carries_an_image_block() {
        let body = "a\n\n![p](x.png)\n\nc\n";
        assert_eq!(moved("mv_img1", body, 4, 0).0, "![p](x.png)\n\na\n\nc\n");
        assert_eq!(
            moved("mv_img2", body, 4, body.len()).0,
            "a\n\nc\n\n![p](x.png)\n"
        );
    }

    #[test]
    fn move_block_carries_a_table() {
        let body = "a\n\n| h |\n|---|\n| c |\n\nc\n";
        assert_eq!(
            moved("mv_tbl1", body, 5, 0).0,
            "| h |\n|---|\n| c |\n\na\n\nc\n"
        );
        assert_eq!(
            moved("mv_tbl2", body, 5, body.len()).0,
            "a\n\nc\n\n| h |\n|---|\n| c |\n"
        );
    }

    #[test]
    fn move_block_carries_a_code_block() {
        let body = "a\n\n```rs\nx\n```\n\nc\n";
        assert_eq!(moved("mv_code1", body, 8, 0).0, "```rs\nx\n```\n\na\n\nc\n");
        assert_eq!(
            moved("mv_code2", body, 8, body.len()).0,
            "a\n\nc\n\n```rs\nx\n```\n"
        );
    }

    #[test]
    fn move_block_onto_its_own_boundary_is_a_quiet_no_op() {
        let mut d = doc_with("mv_noop", "a\n\nb\n");
        let steps = d.undo_steps;
        d.move_block(0, 0);
        d.move_block(0, 3);
        assert_eq!(d.source, "a\n\nb\n");
        assert_eq!(d.undo_steps, steps);
        assert_eq!(d.status, None);
        assert!(!d.dirty);
    }

    #[test]
    fn move_block_into_a_fence_is_refused_with_a_status() {
        let mut d = doc_with("mv_fence", "a\n\n```\nx\ny\n```\n");
        d.move_block(0, 7);
        assert_eq!(d.source, "a\n\n```\nx\ny\n```\n");
        assert!(
            d.status
                .as_deref()
                .is_some_and(|s| s.starts_with("move block"))
        );
    }

    #[test]
    fn move_block_rides_the_caret_with_the_block() {
        // Down: "second" is line 0 of its block, caret 3 bytes from its end.
        let mut d = doc_with("mv_caret1", "first\n\nsecond\n\nthird\n");
        d.caret = 10; // "sec|ond"
        d.move_block(10, d.source.len());
        assert_eq!(d.source, "first\n\nthird\n\nsecond\n");
        assert_eq!(&d.source[d.caret..], "ond\n");
        // Up, across a wrapped paragraph's second line.
        let mut d = doc_with("mv_caret2", "first\n\nsecond\nline two\n");
        d.caret = 16; // "li|ne two"
        d.move_block(16, 0);
        assert_eq!(d.source, "second\nline two\n\nfirst\n");
        assert_eq!(&d.source[d.caret..], "ne two\n\nfirst\n");
        assert_eq!(d.selection(), None);
    }

    #[test]
    fn move_block_keeps_the_caret_on_its_line_through_a_quotes_prefix() {
        let mut d = doc_with("mv_quote_caret", "para\n\n> a\n");
        d.caret = 2; // "pa|ra"
        d.move_block(2, 9); // after a, inside the quote
        assert_eq!(d.source, "> a\n>\n> para\n");
        assert_eq!(&d.source[d.caret..], "ra\n");
    }

    #[test]
    fn move_block_up_and_down_step_over_siblings() {
        let mut d = doc_with("mv_updown", "a\n\nb\n\nc\n");
        d.caret = 3;
        d.move_block_down();
        assert_eq!(d.source, "a\n\nc\n\nb\n");
        assert_eq!(&d.source[d.caret..], "b\n");
        d.move_block_down();
        assert_eq!(d.source, "a\n\nc\n\nb\n", "nothing below the last block");
        assert_eq!(d.status.as_deref(), Some("move block: nothing below"));
        d.move_block_up();
        d.move_block_up();
        assert_eq!(d.source, "b\n\na\n\nc\n");
        assert_eq!(&d.source[d.caret..], "b\n\na\n\nc\n");
        d.move_block_up();
        assert_eq!(d.status.as_deref(), Some("move block: nothing above"));
    }

    #[test]
    fn move_block_up_and_down_reorder_list_items_with_their_children() {
        let mut d = doc_with("mv_items", "- a\n  - x\n- b\n- c\n");
        d.caret = 12; // in "b"
        d.move_block_up();
        assert_eq!(d.source, "- b\n- a\n  - x\n- c\n");
        assert_eq!(&d.source[d.caret..], "b\n- a\n  - x\n- c\n");
        d.caret = 6; // in "a"
        d.move_block_down();
        assert_eq!(d.source, "- b\n- c\n- a\n  - x\n");
        assert_eq!(&d.source[d.caret..], "a\n  - x\n");
        // A nested item leaves its list upward, as an item of the outer one.
        d.caret = 16; // in "x"
        d.move_block_up();
        assert_eq!(d.source, "- b\n- c\n- x\n- a\n");
    }

    #[test]
    fn move_block_on_a_lone_item_that_stays_a_bullet_is_no_step() {
        let mut d = doc_with("mv_lone_item", "x\n\n- b\n\ny\n");
        d.caret = 5;
        let steps = d.undo_steps;
        d.move_block_up();
        assert_eq!(d.source, "x\n\n- b\n\ny\n");
        assert_eq!(
            d.undo_steps, steps,
            "a move that rewrote nothing is no undo step"
        );
        assert_eq!(d.status.as_deref(), Some("move block: nothing above"));
    }

    #[test]
    fn move_block_up_leaves_a_container_at_its_first_block_and_down_at_its_last() {
        let mut d = doc_with("mv_leave", "x\n\n> a\n>\n> b\n\ny\n");
        d.caret = 5; // "a"
        d.move_block_up();
        assert_eq!(d.source, "x\n\na\n\n> b\n\ny\n");
        d.caret = 8; // "b"
        d.move_block_down();
        assert_eq!(d.source, "x\n\na\n\nb\n\ny\n");
        // A tail block leaves its item into the next item's tail, then the list.
        let mut d = doc_with("mv_tail", "- a\n\n  t\n- b\n");
        d.caret = 7;
        d.move_block_down();
        assert_eq!(d.source, "- a\n- b\n\n  t\n");
        d.move_block_down();
        assert_eq!(d.source, "- a\n- b\n\nt\n");
        // And a paragraph after a quote steps over the whole quote, not into it.
        let mut d = doc_with("mv_over", "x\n\n> a\n>\n> b\n\ny\n");
        d.caret = 14;
        d.move_block_up();
        assert_eq!(d.source, "x\n\ny\n\n> a\n>\n> b\n");
        assert_eq!(d.caret, 3);
        d.move_block_down();
        assert_eq!(d.status, None);
        assert_eq!(d.source, "x\n\n> a\n>\n> b\n\ny\n");
        // Above a quote that opens the document is offset 0 — before the
        // quote, not inside it.
        let mut d = doc_with("mv_over_top", "> a\n\ny\n");
        d.caret = 5;
        d.move_block_up();
        assert_eq!(d.source, "y\n\n> a\n");
        assert_eq!(d.caret, 0);
    }

    #[test]
    fn move_block_up_from_the_first_block_of_a_quote_that_opens_the_document_leaves_it() {
        let mut d = doc_with("mv_top_quote", "> a\n>\n> b\n");
        d.caret = 2;
        d.move_block_up();
        assert_eq!(d.source, "a\n\n> b\n");
        assert_eq!(d.caret, 0);
        assert_eq!(d.status, None);
    }

    #[test]
    fn move_block_on_a_blank_line_or_read_only_does_nothing() {
        let mut d = doc_with("mv_blank", "a\n\nb\n");
        d.caret = 2;
        d.move_block_down();
        assert_eq!(d.source, "a\n\nb\n");
        assert_eq!(d.status.as_deref(), Some("move block: no block here"));
        d.read_only = true;
        d.caret = 0;
        d.move_block_down();
        d.move_block(0, 5);
        assert_eq!(d.source, "a\n\nb\n");
    }

    #[test]
    fn move_block_never_lands_above_hidden_frontmatter() {
        let mut d = wysiwyg_doc("mv_fm", "---\nt: x\n---\n\na\n\nb\n");
        let b = d.source.find('b').unwrap();
        d.move_block(b, 0);
        assert_eq!(d.source, "---\nt: x\n---\n\nb\n\na\n");
    }

    #[test]
    fn block_range_at_is_the_block_a_move_picks_up() {
        let mut d = doc_with("mv_range", "a\n\n- b\n  - c\n\nd\n");
        assert_eq!(d.block_range_at(0), Some(0..1));
        assert_eq!(d.block_range_at(5), Some(3..12), "the item with its child");
        assert_eq!(d.block_range_at(10), Some(7..12), "the nested item alone");
        assert_eq!(d.block_range_at(2), None, "a blank line");
    }

    #[test]
    fn drop_target_at_splits_a_block_at_its_middle_row_and_ends_below_everything() {
        let long = "two ".repeat(40).trim_end().to_string(); // wraps to three rows at 80
        let mut d = wysiwyg_doc("drop", &format!("one\n\n{long} four\n\nfive\n"));
        let rows = d.vmap.rows.len();
        assert_eq!(rows, 7, "one, gap, three wrapped rows, gap, five");
        assert_eq!(d.drop_target_at(0), Some(DropTarget { offset: 0, row: 0 }));
        let two = d.source.find("two").unwrap();
        assert_eq!(
            d.drop_target_at(1),
            Some(DropTarget {
                offset: two,
                row: 2
            }),
            "the gap resolves to the block under it"
        );
        assert_eq!(
            d.drop_target_at(2),
            Some(DropTarget {
                offset: two,
                row: 2
            })
        );
        assert_eq!(
            d.drop_target_at(3),
            Some(DropTarget {
                offset: two,
                row: 2
            })
        );
        let four_end = d.source.find("four").unwrap() + 4;
        assert_eq!(
            d.drop_target_at(4),
            Some(DropTarget {
                offset: four_end,
                row: 5
            })
        );
        assert_eq!(
            d.drop_target_at(rows + 3),
            Some(DropTarget {
                offset: d.source.len(),
                row: rows
            })
        );
        // And the offsets are ones move_block takes.
        let five = d.source.find("five").unwrap();
        let t = d.drop_target_at(2).unwrap();
        d.move_block(five, t.offset);
        assert_eq!(d.source, format!("one\n\nfive\n\n{long} four\n"));
    }

    #[test]
    fn append_media_lands_at_the_end_as_its_own_block() {
        let mut d = doc_with("append_mid", "first word and more\n");
        d.caret = 0; // an editor nobody has tapped: the caret is at the start
        d.append_media(MediaKind::Image, "cat.png", "");
        assert_eq!(d.source, "first word and more\n\n![](cat.png)");
        assert_eq!(d.selection(), None);
        assert_eq!(d.caret, "first word and more\n\n![](cat.png)".len());
    }

    #[test]
    fn append_media_needs_no_separator_after_a_blank_line_or_in_an_empty_document() {
        let mut d = doc_with("append_blank", "para\n\n");
        d.append_media(MediaKind::Image, "a.png", "");
        assert_eq!(d.source, "para\n\n![](a.png)");

        let mut e = doc_with("append_empty", "");
        e.append_media(MediaKind::Image, "b.png", "");
        assert_eq!(e.source, "![](b.png)");

        let mut f = doc_with("append_noeol", "no newline at end");
        f.anchor = Some(0);
        f.caret = 2; // a selection, which the verb ignores
        f.append_media(MediaKind::Video, "clip.mp4", "");
        assert_eq!(
            f.source,
            "no newline at end\n\n<video src=\"clip.mp4\" controls></video>"
        );
    }

    #[test]
    fn insert_media_spells_a_video_as_html_and_reads_it_back_as_a_block() {
        // The round trip is the point: it's no use writing markup the reader
        // can't pick up again. This is the pair that only holds from twig 2.5.1
        // on — before it, the one-line form went in fine and came back as a
        // paragraph of raw tags, publishing no media at all.
        let mut d = doc_with("vid_rt", "\n");
        d.caret = 0;
        d.insert_media(MediaKind::Video, "clip.mp4", "a clip");
        assert_eq!(
            d.source,
            "<video src=\"clip.mp4\" controls>a clip</video>\n"
        );

        d.build_visual(80);
        assert_eq!(d.vmap.media.len(), 1, "reads back as one block media");
        assert_eq!(d.vmap.media[0].kind, MediaKind::Video);
        assert_eq!(d.vmap.media[0].destination, "clip.mp4");
        assert_eq!(d.vmap.media[0].alt, "a clip");
    }

    #[test]
    fn insert_media_spells_audio_with_its_own_tag() {
        let mut d = doc_with("aud_rt", "\n");
        d.caret = 0;
        d.insert_media(MediaKind::Audio, "take.mp3", "");
        assert_eq!(d.source, "<audio src=\"take.mp3\" controls></audio>\n");
        d.build_visual(80);
        assert_eq!(d.vmap.media[0].kind, MediaKind::Audio);
    }

    #[test]
    fn insert_media_uses_the_selection_as_fallback_text() {
        // The same courtesy `insert_image` does with alt: select a caption,
        // insert, and the caption labels the thing rather than being replaced.
        let mut d = doc_with("vid_sel", "the talk here\n");
        d.anchor = Some(0);
        d.caret = 8; // "the talk"
        d.insert_media(MediaKind::Video, "talk.mp4", "ignored fallback");
        assert_eq!(
            d.source,
            "<video src=\"talk.mp4\" controls>the talk</video> here\n"
        );
    }

    #[test]
    fn insert_media_with_an_image_kind_is_just_insert_image() {
        let mut d = doc_with("img_via_media", "\n");
        d.caret = 0;
        d.insert_media(MediaKind::Image, "logo.svg", "x");
        assert_eq!(d.source, "![x](logo.svg)\n");
    }

    // ── thematic breaks ─────────────────────────────────────────────────────

    /// The node the source parses as at `caret` — what confirms an inserted
    /// `---` actually reads back as a rule, not stray text or a setext heading.
    ///
    /// The *narrowest* node covering the offset. Every ancestor covers it too,
    /// and since twig 2.8 that includes the `doc` root, which now carries a real
    /// span (it reported none before, so taking the first match used to land on
    /// the block by luck and now always answers `"doc"`).
    fn kind_at(d: &mut Doc, caret: usize) -> Option<Kind> {
        d.nodes()
            .into_iter()
            .filter(|n| n.span.start <= caret && caret < n.span.end)
            .min_by_key(|n| n.span.end - n.span.start)
            .map(|n| n.kind)
    }

    #[test]
    fn a_task_box_toggles_at_the_caret_and_reads_back() {
        let mut d = doc_with("task_toggle", "- [ ] todo\n- [x] done\n");
        d.caret = 8; // inside "todo"
        assert_eq!(d.task_checked_at_caret(), Some(false));
        d.toggle_task_checked();
        assert_eq!(d.source, "- [x] todo\n- [x] done\n");
        assert_eq!(d.task_checked_at_caret(), Some(true));
        d.toggle_task_checked();
        assert_eq!(d.source, "- [ ] todo\n- [x] done\n");
    }

    #[test]
    fn a_click_toggles_a_box_without_taking_the_caret_with_it() {
        // The whole reason `toggle_task_at` exists apart from the caret form:
        // ticking a box elsewhere must not move the cursor out of what's being
        // typed.
        let mut d = doc_with("task_click", "- [ ] first\n- [ ] second\n");
        d.caret = 8; // inside "first"
        let second = d.source.find("second").unwrap();
        d.toggle_task_at(second);
        assert_eq!(d.source, "- [ ] first\n- [x] second\n");
        assert_eq!(d.caret, 8, "the caret stayed in the first item");
    }

    #[test]
    fn a_paragraph_becomes_a_task_item_in_one_undo_step() {
        let mut d = doc_with("task_para", "first\n\nplain para\n");
        d.caret = d.source.find("para").unwrap();
        d.toggle_task_item();
        assert_eq!(d.source, "first\n\n- [ ] plain para\n");
        assert_eq!(d.task_checked_at_caret(), Some(false));
        assert_eq!(d.status, None);
        d.undo();
        assert_eq!(
            d.source, "first\n\nplain para\n",
            "one step takes both back"
        );
    }

    #[test]
    fn a_blank_line_becomes_an_empty_task_item() {
        let mut d = doc_with("task_blank", "first\n\n");
        d.caret = d.source.len();
        d.toggle_task_item();
        assert_eq!(d.task_checked_at_caret(), Some(false), "{:?}", d.source);
        assert_eq!(d.status, None);
    }

    #[test]
    fn a_plain_item_gains_and_loses_a_box() {
        let mut d = doc_with("task_mint", "- plain\n");
        d.caret = 4;
        assert_eq!(d.task_checked_at_caret(), None);
        d.toggle_task_item();
        assert_eq!(d.source, "- [ ] plain\n");
        assert_eq!(
            d.task_checked_at_caret(),
            Some(false),
            "a new box arrives unticked"
        );
        d.toggle_task_item();
        assert_eq!(d.source, "- plain\n");
    }

    #[test]
    fn ticking_a_box_that_isnt_there_reports_rather_than_minting_one() {
        // `set checked` must not silently convert a bullet into a task — that is
        // `toggle_task_item`'s job, and twig refuses it here.
        let mut d = doc_with("task_none", "- plain\n");
        d.caret = 4;
        d.toggle_task_checked();
        assert_eq!(d.source, "- plain\n", "nothing written");
        assert!(
            d.status.is_some(),
            "the refusal should reach the status line"
        );
    }

    #[test]
    fn a_task_item_in_a_quote_is_found_past_the_quote_marker() {
        let mut d = doc_with("task_quote", "> - [ ] nested\n");
        d.caret = d.source.find("nested").unwrap();
        assert_eq!(d.task_checked_at_caret(), Some(false));
        d.toggle_task_checked();
        assert_eq!(d.source, "> - [x] nested\n");
    }

    #[test]
    fn insert_thematic_break_parts_the_paragraph_around_the_caret() {
        // A rule is a block, so twig's `insert_thematic_break` alone lands it
        // after the whole paragraph. `split_block` parts the paragraph first and
        // the rule is aimed at the *first* half, which is what a rule button is
        // understood to do — and what leaf spelled by hand until twig grew both
        // halves of the gesture.
        let mut d = doc_with("hr_mid", "before after\n");
        d.caret = 7; // between "before " and "after"
        d.insert_thematic_break();
        assert_eq!(d.source, "before \n\n---\n\nafter\n");
        assert_eq!(d.selection(), None);
        assert_eq!(
            kind_at(&mut d, "before \n\n".len()),
            Some(Kind::ThematicBreak)
        );
    }

    #[test]
    fn insert_thematic_break_at_a_paragraph_s_end_splits_nothing() {
        // At the end there is nothing to part, and a split there writes the
        // separator anyway — a blank line and the empty slot the next paragraph
        // would fill — which the rule then landed above: `para\n\n* * *\n\n\n`,
        // two blank lines nothing fills. Now the rule lands after the paragraph,
        // where the split-and-aim was sending it regardless. Both formats, and
        // both shapes of a last line — terminated, and still being typed —
        // because the two reach the split through different doors: Markdown's
        // paragraph span stops before its newline, so `para\n` at 4 never split
        // there, but `para` at 4 did.
        //
        // What stands under the rule is the line the caret goes on to, not a
        // slot: nothing above the rule but the one blank line, and the caret on
        // the line beneath it.
        for (fmt, rule) in [(Format::Markdown, "---"), (Format::Djot, "* * *")] {
            for src in ["para\n", "para"] {
                let mut d = Doc::from_source(src.into(), fmt).unwrap();
                d.caret = 4;
                d.insert_thematic_break();
                assert_eq!(d.source, format!("para\n\n{rule}\n\n"), "{fmt:?} {src:?}");
                assert_eq!(d.caret, d.source.len());
            }
            // Mid-document the slot sat between the rule and the next block;
            // the caret's line does now, a blank line either side of it.
            let mut d = Doc::from_source("para\n\nnext\n".into(), fmt).unwrap();
            d.caret = 4;
            d.insert_thematic_break();
            assert_eq!(d.source, format!("para\n\n{rule}\n\n\n\nnext\n"), "{fmt:?}");
            // Trailing whitespace is nothing to part either.
            let mut d = Doc::from_source("para  \n".into(), fmt).unwrap();
            d.caret = 4;
            d.insert_thematic_break();
            assert_eq!(d.source, format!("para  \n\n{rule}\n\n"), "{fmt:?}");
        }
    }

    #[test]
    fn insert_thematic_break_at_a_paragraph_s_start_lands_before_it() {
        // The split at the start parts nothing, but it is kept on purpose:
        // `|para` becomes `\npara` with the caret on a blank line, and twig
        // (3.5.2) writes a rule aimed at a blank line ON that line — the only
        // way "before the paragraph" is reachable through a gesture that only
        // places after. Before 3.5.2 this came out as `\n\n---\n\npara`.
        for (fmt, rule) in [(Format::Markdown, "---"), (Format::Djot, "* * *")] {
            let mut d = Doc::from_source("para\n".into(), fmt).unwrap();
            d.caret = 0;
            d.insert_thematic_break();
            assert_eq!(d.source, format!("{rule}\n\npara\n"), "{fmt:?}");
            let mut d = Doc::from_source("prev\n\npara\n".into(), fmt).unwrap();
            d.caret = 6;
            d.insert_thematic_break();
            assert_eq!(d.source, format!("prev\n\n{rule}\n\npara\n"), "{fmt:?}");
        }
    }

    #[test]
    fn insert_thematic_break_on_a_blank_line_takes_that_line() {
        // The gap between two blocks is where a click lands the caret; the
        // rule goes on the blank, one blank each side, and the caret on the
        // line opened under it.
        let mut d = doc_with("hr_gap", "a\n\nb\n");
        d.caret = 2;
        d.insert_thematic_break();
        assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
        assert_eq!(d.caret, "a\n\n---\n\n".len());
    }

    #[test]
    fn insert_thematic_break_leaves_the_caret_on_a_line_under_the_rule() {
        // The caret used to be left where twig's splice ended, the home past
        // the rule, which is drawn on the rule's own row: the writer saw the
        // caret beside the rule, and mid-document what they typed next ran
        // into the block below. Now it goes on to an empty row under the
        // rule's gap from every place a rule is asked for — the end of a
        // paragraph, the empty paragraph Enter opened, a list and the line
        // Enter leaves a list for, closing the document and before another
        // block — and what is typed there is a paragraph of its own.
        for (body, caret, typed) in [
            ("para\n", 4, "para\n\n---\n\nX"),
            ("para\n\n\n", 6, "para\n\n---\n\nX"),
            ("- one\n- two\n", 11, "- one\n- two\n\n---\n\nX"),
            ("- one\n- two\n\n\n", 13, "- one\n- two\n\n---\n\nX"),
            ("a\n\nb\n", 1, "a\n\n---\n\nX\n\nb\n"),
            ("a\n\n\n\nb\n", 3, "a\n\n---\n\nX\n\nb\n"),
        ] {
            let mut d = wysiwyg_doc("hr_caret", body);
            d.build_visual(80);
            d.caret = caret;
            d.insert_thematic_break();
            d.build_visual(80);
            let rule = d.vmap.pos_of_offset(d.source.find("---").unwrap()).0;
            let (row, col) = d.vmap.pos_of_offset(d.caret);
            assert_eq!((row, col), (rule + 2, 0), "{body:?} → {:?}", d.source);
            assert!(d.vmap.rows[rule + 1].decoration, "the gap under the rule");
            assert!(d.vmap.rows[row].glyphs.is_empty(), "an empty line");
            d.insert("X");
            assert_eq!(d.source, typed, "{body:?}");
            let rule_at = d.source.find("---").unwrap();
            assert_eq!(kind_at(&mut d, rule_at), Some(Kind::ThematicBreak));
        }
        // djot's rule takes in its newline; the line under it is the same.
        let mut d = Doc::from_source("a\n\nb\n".into(), Format::Djot).unwrap();
        d.view = View::Wysiwyg;
        d.caret = 1;
        d.insert_thematic_break();
        d.build_visual(80);
        assert_eq!(d.source, "a\n\n* * *\n\n\n\nb\n");
        assert_eq!(d.vmap.pos_of_offset(d.caret), (4, 0));
    }

    #[test]
    fn a_rule_parting_a_paragraph_leaves_the_caret_before_its_second_half() {
        // The line under the rule is the paragraph's second half, so that is
        // where the caret goes: the text it was standing in front of is still
        // in front of it.
        let mut d = wysiwyg_doc("hr_parted", "before after\n");
        d.caret = 7;
        d.insert_thematic_break();
        assert_eq!(d.source, "before \n\n---\n\nafter\n");
        assert_eq!(d.caret, d.source.find("after").unwrap());
        // At the start of a paragraph too, the rule landing above it.
        let mut d = wysiwyg_doc("hr_parted_start", "prev\n\npara\n");
        d.caret = 6;
        d.insert_thematic_break();
        assert_eq!(d.caret, d.source.find("para").unwrap());
    }

    #[test]
    fn the_line_under_a_new_rule_is_one_undo_step_with_it() {
        let mut d = wysiwyg_doc("hr_undo", "a\n\nb\n");
        d.caret = 1;
        d.insert_thematic_break();
        assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
        d.undo();
        assert_eq!(d.source, "a\n\nb\n");
        assert!(!d.can_undo());
        d.redo();
        assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
    }

    #[test]
    fn a_rule_in_preserve_flow_leaves_the_caret_on_the_first_line_under_it() {
        // Preserve draws every blank line as somewhere to type, the first under
        // the rule included, so that is the caret's line and the one written.
        let mut d = wysiwyg_doc("hr_preserve", "para\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 4;
        d.insert_thematic_break();
        assert_eq!(d.source, "para\n\n---\n\n");
        assert_eq!(d.caret, "para\n\n---\n".len());
        d.build_visual(80);
        let rule = d.vmap.pos_of_offset(d.source.find("---").unwrap()).0;
        assert_eq!(d.vmap.pos_of_offset(d.caret), (rule + 1, 0));
        // Before another block, one blank line more keeps it off that block.
        let mut d = wysiwyg_doc("hr_preserve_mid", "a\n\nb\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = 1;
        d.insert_thematic_break();
        assert_eq!(d.source, "a\n\n---\n\n\nb\n");
        assert_eq!(d.caret, "a\n\n---\n".len());
    }

    #[test]
    fn insert_page_break_leaves_the_caret_on_a_line_under_it() {
        // The rule button's placement, for the other block the toolbar writes:
        // the caret was left at the end of twig's splice, beside the break at
        // the end of the document and on the next block's first letter before
        // one. djot spells the break as a fence of two lines; the line under
        // it is under the closing `:::`.
        for (fmt, body, want, typed) in [
            (
                Format::Markdown,
                "a\n\nb\n",
                "a\n\n::page-break\n\n\n\nb\n",
                "a\n\n::page-break\n\nX\n\nb\n",
            ),
            (
                Format::Markdown,
                "a\n",
                "a\n\n::page-break\n\n",
                "a\n\n::page-break\n\nX",
            ),
            (
                Format::Djot,
                "a\n\nb\n",
                "a\n\n::: page-break\n:::\n\n\n\nb\n",
                "a\n\n::: page-break\n:::\n\nX\n\nb\n",
            ),
        ] {
            let mut d = Doc::from_source(body.into(), fmt).unwrap();
            d.view = View::Wysiwyg;
            d.caret = 1;
            d.insert_page_break();
            assert_eq!(d.source, want, "{fmt:?}");
            d.build_visual(80);
            let (row, col) = d.vmap.pos_of_offset(d.caret);
            assert_eq!(col, 0);
            assert!(d.vmap.rows[row].glyphs.is_empty(), "{fmt:?}: an empty line");
            assert!(
                d.vmap.rows[row - 2].leaf_directive.is_some(),
                "{fmt:?}: under the break"
            );
            d.insert("X");
            assert_eq!(d.source, typed, "{fmt:?}");
            d.undo();
            d.undo();
            assert_eq!(
                d.source, body,
                "{fmt:?}: the break and its line are one step"
            );
        }
    }

    #[test]
    fn a_block_parting_a_paragraph_is_one_undo_step() {
        // The split went to twig without leaf counting it, so the first Undo
        // took back the block, left the paragraph parted, and reported nothing
        // more to undo.
        let mut rule = wysiwyg_doc("part_undo_rule", "before after\n");
        rule.caret = 7;
        rule.insert_thematic_break();
        let mut table = wysiwyg_doc("part_undo_table", "before after\n");
        table.caret = 7;
        table.insert_table(1, 1);
        let mut page = wysiwyg_doc("part_undo_page", "before after\n");
        page.caret = 7;
        page.insert_page_break();
        for (name, d) in [
            ("rule", &mut rule),
            ("table", &mut table),
            ("page break", &mut page),
        ] {
            assert_ne!(d.source, "before after\n", "{name}");
            d.undo();
            assert_eq!(d.source, "before after\n", "{name}: one Undo");
            assert_eq!(d.caret, 7, "{name}: the caret back where it was");
            assert!(!d.can_undo(), "{name}");
            assert!(d.can_redo(), "{name}");
            d.redo();
            assert!(!d.source.starts_with("before after"), "{name}: one Redo");
            assert!(!d.can_redo(), "{name}");
        }
        // A table twig would refuse parts nothing: the refusal comes first.
        let mut d = wysiwyg_doc("part_undo_zero", "before after\n");
        d.caret = 7;
        d.insert_table(0, 1);
        assert_eq!(d.source, "before after\n");
        assert!(!d.can_undo());
    }

    #[test]
    fn enter_at_a_paragraph_s_start_opens_an_empty_paragraph_above_it() {
        // twig's split at a paragraph's start writes one newline ahead of it,
        // which Fold draws as one more gap, so the first Enter showed nothing.
        // A paragraph break opens a line above, the caret staying before the
        // text. Past a mark's opening delimiters — where a tap before the
        // first letter lands — the split used to cut the mark in two
        // (`**\n\nb**`); the break goes in front of them, as it goes in
        // front of a heading's `#`, where the heading's own Enter left an
        // empty heading over its text as a paragraph. A paragraph that
        // opens a div gets its line outside the div, where it draws.
        for (body, caret, want, want_caret) in [
            ("a\n\nb\n", 3, "a\n\n\n\nb\n", 5),
            ("# H\n\nb\n", 5, "# H\n\n\n\nb\n", 7),
            ("a\n\n# H\n", 5, "a\n\n\n\n# H\n", 7),
            ("a\n\n**b** c\n", 3, "a\n\n\n\n**b** c\n", 5),
            ("a\n\n**b** c\n", 5, "a\n\n\n\n**b** c\n", 7),
            (
                "a\n\n<span data-color=\"orange\">b</span>\n",
                29,
                "a\n\n\n\n<span data-color=\"orange\">b</span>\n",
                31,
            ),
            (
                "a\n\n<div class=\"center\">\n\nb\n\n</div>\n",
                25,
                "a\n\n\n\n<div class=\"center\">\n\nb\n\n</div>\n",
                27,
            ),
        ] {
            let mut d = wysiwyg_doc("enter_para_start", body);
            d.build_visual(80);
            d.caret = caret;
            d.newline();
            assert_eq!(d.source, want, "{body:?}");
            assert_eq!(d.caret, want_caret, "{body:?}");
            d.build_visual(80);
            let (row, _) = d.vmap.pos_of_offset(d.caret);
            assert!(
                d.vmap.rows[row - 2].glyphs.is_empty(),
                "{body:?}: an empty line above"
            );
            assert!(
                !d.vmap.rows[row - 2].decoration,
                "{body:?}: one the caret can stand on"
            );
        }
        // Past the first letter it is an ordinary split.
        let mut d = wysiwyg_doc("enter_para_mid", "a\n\n**b** c\n");
        d.caret = 8;
        d.newline();
        assert_eq!(d.source, "a\n\n**b**\n\nc\n");
    }

    #[test]
    fn enter_again_at_a_paragraph_s_start_opens_one_more_line() {
        // The first Enter's paragraph break leaves a gap either side of the
        // empty line. A second break drew two lines more; a newline draws one.
        let mut d = wysiwyg_doc("enter_para_again", "a\n\nb\n");
        d.build_visual(80);
        d.caret = 3;
        for (want, lines) in [("a\n\n\n\nb\n", 1), ("a\n\n\n\n\nb\n", 2)] {
            d.newline();
            assert_eq!(d.source, want);
            assert_eq!(&d.source[d.caret..], "b\n");
            d.build_visual(80);
            let empty = d
                .vmap
                .rows
                .iter()
                .filter(|r| r.glyphs.is_empty() && !r.decoration)
                .count();
            assert_eq!(empty, lines, "{want:?}");
        }
    }

    #[test]
    fn enter_at_the_first_block_s_start_pushes_it_down() {
        // No row was drawn for a blank line above the first block, so Enter
        // there wrote a line and the text stayed where it was. Each Enter now
        // draws one empty line above it, the caret staying with the text;
        // past frontmatter, the line conventionally under it still draws
        // nothing until Enter adds to it.
        for (body, caret, wants) in [
            ("b\n", 0, ["\n\nb\n", "\n\n\nb\n"]),
            ("**b** c\n", 2, ["\n\n**b** c\n", "\n\n\n**b** c\n"]),
            ("# H\n", 2, ["\n\n# H\n", "\n\n\n# H\n"]),
            (
                "---\nt: x\n---\n\nb\n",
                14,
                ["---\nt: x\n---\n\n\nb\n", "---\nt: x\n---\n\n\n\nb\n"],
            ),
        ] {
            let mut d = wysiwyg_doc("enter_first_block", body);
            d.build_visual(80);
            let empty = |d: &Doc| {
                d.vmap
                    .rows
                    .iter()
                    .filter(|r| r.glyphs.is_empty() && !r.decoration)
                    .count()
            };
            assert_eq!(empty(&d), 0, "{body:?}: nothing above the text yet");
            d.caret = caret;
            for (lines, want) in wants.into_iter().enumerate() {
                d.newline();
                assert_eq!(d.source, want, "{body:?}");
                assert!(d.source[d.caret..].starts_with(&body[caret..]), "{body:?}");
                d.build_visual(80);
                assert_eq!(empty(&d), lines + 1, "{want:?}");
            }
            // The caret can go up onto the lines it opened.
            d.move_up(false);
            let (row, _) = d.vmap.pos_of_offset(d.caret);
            assert!(d.vmap.rows[row].glyphs.is_empty(), "{body:?}");
            assert!(!d.vmap.rows[row].decoration, "{body:?}");
        }
    }

    /// What a reader sees of a map: each row's text, whether it is a gap,
    /// and the presentation it is drawn with. Offsets are left out, since an
    /// edit shifts them without anything on screen changing.
    fn drawn(d: &Doc) -> Vec<String> {
        d.vmap
            .rows
            .iter()
            .map(|r| {
                let text: String = r.glyphs.iter().map(|g| g.ch).collect();
                format!(
                    "{}{text:?} h={:?} lh={:?} a={:?}",
                    if r.decoration { "~" } else { "" },
                    r.heading,
                    r.line_height,
                    r.align
                )
            })
            .collect()
    }

    #[test]
    fn enter_and_backspace_anywhere_show_what_they_write_and_undo_in_one_step() {
        // Enter that writes what the view does not draw looks like a key that
        // did nothing, so it gets pressed again, and the lines pile up unseen
        // until something redraws them all at once. The rule, the page break,
        // a paragraph's start, the first block's start and the blank page
        // were each that bug once. So: at every place the caret can stand, in
        // both flows, Enter either changes what is drawn or writes nothing,
        // and one Undo takes back whatever it wrote. Enter and Backspace both
        // leave the view a fresh open of the text would draw, so nothing
        // moves when the view is next rebuilt from scratch.
        let cases: &[(Format, &str)] = &[
            (
                Format::Markdown,
                "I talk to myself\n\nI laugh\n\nI cry\n\nknees to nose\n\nlet me go\n",
            ),
            (Format::Markdown, "I talk to myself\nI laugh\nI cry\n"),
            (
                Format::Markdown,
                "<div data-line-height=\"1.5\">\n\nI laugh\n\nI cry\n\nknees to nose\n\n</div>\n",
            ),
            (
                Format::Markdown,
                "<div data-line-height=\"1.15\">\n\nI cry\n\n</div>\n\n<div data-line-height=\"1.15\">\n\nknees\n\n</div>\n",
            ),
            (
                Format::Markdown,
                "<div class=\"center\">\n\nmiddle\n\n</div>\n\nafter\n",
            ),
            (
                Format::Markdown,
                "---\ntitle: x\n---\n\n# Title\n\nbody **bold** and <span data-color=\"red\">red</span>\n",
            ),
            (Format::Markdown, "\n\n\nafter blank lines\n\n\n\nmid\n\n\n"),
            (
                Format::Markdown,
                "- one\n- two\n\n1. a\n2. b\n\n- [ ] task\n",
            ),
            (Format::Markdown, "> quoted\n> more\n\n> - in a quote\n"),
            (Format::Markdown, "a\n\n---\n\nb\n\n::page-break\n\nc\n"),
            (Format::Markdown, "Title\n=====\n\nSub\n---\n\ntext\n"),
            (
                Format::Markdown,
                "```\ncode\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            ),
            (Format::Djot, "I laugh\n\nI cry\n\nknees to nose\n"),
            (
                Format::Djot,
                "{data-line-height=\"2\"}\nI cry\n\n{data-line-height=\"2\"}\nknees to nose\n",
            ),
            (Format::Djot, "# Title\n\na\n\n* * *\n\nb\n"),
            // HTML is not here: Enter writes blank lines there, which the
            // view draws and HTML does not keep. See
            // docs/tasks/enter-in-html-writes-whitespace.md.
        ];
        let mut failures = Vec::new();
        for &(format, body) in cases {
            for flow in [LineFlow::Fold, LineFlow::Preserve] {
                let open = || {
                    let mut d = Doc::from_source(body.into(), format).unwrap();
                    d.view = View::Wysiwyg;
                    d.set_line_flow(flow);
                    d.build_visual_unwrapped();
                    d
                };
                // The map a fresh open of the same text would draw. The
                // edited map differing from it is a view that changes on the
                // next full rebuild, seconds after the key, with no key.
                let fresh = |source: &str| {
                    let mut ed =
                        twig::Editor::new_ext(source.as_bytes(), format, parse_extensions())
                            .unwrap();
                    let nodes = ed.nodes().unwrap();
                    crate::wysiwyg::build(
                        &nodes,
                        source,
                        None,
                        flow == LineFlow::Preserve,
                        &wysiwyg::Surface::default(),
                        None,
                    )
                };
                let probe = open();
                let stops: Vec<usize> = (0..=body.len())
                    .filter(|&o| probe.vmap.is_stop(o))
                    .collect();
                for at in stops {
                    for enter in [true, false] {
                        let mut d = open();
                        d.caret = at;
                        d.anchor = None;
                        let before = drawn(&d);
                        if enter {
                            d.newline();
                        } else {
                            d.backspace();
                        }
                        d.build_visual_unwrapped();
                        let key = if enter { "enter" } else { "backspace" };
                        let what = format!(
                            "{key} {format:?} {flow:?} caret {at} in {body:?} -> {:?}",
                            d.source
                        );
                        if maps_differ(&d.vmap, &fresh(&d.source)) {
                            failures.push(format!("redraws differently: {what}"));
                        }
                        if d.source == body {
                            continue;
                        }
                        if enter && drawn(&d) == before {
                            failures.push(format!("drew nothing: {what}"));
                        }
                        d.undo();
                        if d.source != body {
                            failures.push(format!("undo left {:?}: {what}", d.source));
                        }
                    }
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn enter_opens_a_line_that_draws_inside_a_div_a_quote_and_preserve_flow() {
        // Each of these wrote a line the view did not draw, so Enter looked
        // dead and every press left one more.
        let enter = |format, flow, body: &str, at: usize| {
            let mut d = Doc::from_source(body.into(), format).unwrap();
            d.view = View::Wysiwyg;
            d.set_line_flow(flow);
            d.build_visual(80);
            d.caret = at;
            d.newline();
            d.build_visual(80);
            d
        };
        let div = "<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\n</div>\n";
        let end = div.find("knees").unwrap() + "knees".len();

        // At the end of a div's last paragraph, the empty paragraph opens
        // inside the div, drawn with its line height, and what is typed
        // there stays in it.
        let mut d = enter(Format::Markdown, LineFlow::Fold, div, end);
        let (row, _) = d.vmap.pos_of_offset(d.caret);
        let r = &d.vmap.rows[row];
        assert!(r.glyphs.is_empty() && !r.decoration, "{:?}", drawn(&d));
        assert!(
            r.line_height.is_some(),
            "the new line keeps the div's spacing"
        );
        d.insert("x");
        assert_eq!(
            d.source,
            "<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\nx\n\n</div>\n"
        );
        // Preserve flow's soft line there draws too.
        let d = enter(Format::Markdown, LineFlow::Preserve, div, end);
        assert_eq!(
            d.source,
            "<div data-line-height=\"1.5\">\n\nI cry\n\nknees\n\n\n</div>\n"
        );
        assert_eq!(d.vmap.rows.len(), 4, "{:?}", drawn(&d));
        // Between two of the div's paragraphs, the empty line takes its
        // spacing as well.
        let d = enter(
            Format::Markdown,
            LineFlow::Fold,
            div,
            div.find("knees").unwrap(),
        );
        let (row, _) = d.vmap.pos_of_offset(d.caret);
        assert!(
            d.vmap.rows[row - 2].line_height.is_some(),
            "{:?}",
            drawn(&d)
        );

        // Under `</div>` the blank line is the one Markdown needs to end the
        // div. Preserve flow drew it as a line to type on, and Backspace
        // there glued the next paragraph to the tag (`</div>after`).
        let below = "<div class=\"center\">\n\nmiddle\n\n</div>\n\nafter\n";
        let mut d = Doc::from_source(below.into(), Format::Markdown).unwrap();
        d.view = View::Wysiwyg;
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
        assert_eq!(d.vmap.rows.len(), 2, "{:?}", drawn(&d));
        d.caret = below.find("after").unwrap();
        d.backspace();
        assert!(!d.source.contains("</div>after"), "{:?}", d.source);

        // At a quote's first line, the line opens above the quote; twig's
        // split wrote a quoted blank line above the text, which drew nothing.
        let d = enter(
            Format::Markdown,
            LineFlow::Fold,
            "a\n\n> quoted\n> more\n",
            5,
        );
        assert_eq!(d.source, "a\n\n\n\n> quoted\n> more\n");
        assert_eq!(&d.source[d.caret..], "quoted\n> more\n");

        // Preserve flow at a paragraph's start: past a mark's delimiters its
        // newline cut the mark (`**\nb**`), and in djot a heading ran on
        // over it unseen.
        let d = enter(Format::Markdown, LineFlow::Preserve, "a\n\n**b** c\n", 5);
        assert_eq!(d.source, "a\n\n\n**b** c\n");
        let d = enter(Format::Djot, LineFlow::Preserve, "a\n\n# Title\n", 5);
        assert_eq!(d.source, "a\n\n\n# Title\n");
        // Inside a heading it parts the heading, as in Fold flow.
        let d = enter(Format::Djot, LineFlow::Preserve, "a\n\n# Title\n", 7);
        assert_eq!(d.source, "a\n\n# Ti\n\ntle\n");
    }

    #[test]
    fn enter_on_a_blank_page_writes_nothing() {
        // With no block to push down, Fold flow draws no line for Enter to
        // open, and the newline it wrote was an edit and an undo step that
        // showed nothing. Preserve flow draws every line, so there it writes.
        for body in ["", "\n", "\n\n  \n", "---\nt: x\n---\n"] {
            let mut d = wysiwyg_doc("enter_blank_page", body);
            d.build_visual(80);
            d.caret = d.source.len();
            d.newline();
            assert_eq!(d.source, body);
            assert!(!d.can_undo(), "{body:?}");
            assert!(!d.dirty, "{body:?}");
        }
        let mut d = wysiwyg_doc("enter_blank_page_preserve", "");
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
        d.newline();
        assert_eq!(d.source, "\n");
    }

    #[test]
    fn enter_beside_a_rule_opens_a_line_under_it() {
        // Beside a rule the caret stands at the home past it, which in source is
        // the start of the blank line under the rule, and Enter took it for an
        // empty paragraph: a lone newline, drawn as one more gap, and the caret
        // put on the next block's first line — where typing joined that block.
        // It goes on under the rule instead, as the rule button leaves it.
        let mut d = wysiwyg_doc("enter_rule", "a\n\n---\n\nb\n");
        d.build_visual(80);
        d.caret = "a\n\n---\n".len();
        d.newline();
        assert_eq!(d.source, "a\n\n---\n\n\n\nb\n");
        assert_eq!(d.caret, "a\n\n---\n\n".len());
        d.insert("X");
        assert_eq!(d.source, "a\n\n---\n\nX\n\nb\n");

        // Closing the document.
        let mut d = wysiwyg_doc("enter_rule_end", "a\n\n---\n");
        d.caret = d.source.len();
        d.newline();
        assert_eq!(d.source, "a\n\n---\n\n");
        assert_eq!(d.caret, d.source.len());

        // In a quote, where the lines it writes keep the quote's prefix.
        let mut d = wysiwyg_doc("enter_rule_quote", "> a\n>\n> ---\n>\n> b\n");
        d.caret = "> a\n>\n> ---\n".len();
        d.newline();
        assert_eq!(d.source, "> a\n>\n> ---\n>\n> \n>\n> b\n");
        d.insert("X");
        assert_eq!(d.source, "> a\n>\n> ---\n>\n> X\n>\n> b\n");

        // Preserve draws the blank line under a rule as a line, and the caret
        // on it: Enter there is a blank line's, one line down.
        let mut d = wysiwyg_doc("enter_rule_preserve", "a\n\n---\n\nb\n");
        d.set_line_flow(LineFlow::Preserve);
        d.caret = "a\n\n---\n".len();
        d.newline();
        assert_eq!(d.source, "a\n\n---\n\n\nb\n");
        assert_eq!(d.caret, "a\n\n---\n\n".len());
    }

    #[test]
    fn insert_table_at_a_paragraph_s_end_splits_nothing() {
        // The same door as the rule's, through the placement they share.
        let mut d = Doc::from_source("para\n".into(), Format::Djot).unwrap();
        d.caret = 4;
        d.insert_table(1, 1);
        assert_eq!(d.source, "para\n\n|  |\n|---|\n|  |\n");
        let mut d = doc_with("table_end_typed", "para");
        d.caret = 4;
        d.insert_table(1, 1);
        assert_eq!(d.source, "para\n\n|  |\n| --- |\n|  |\n");
        assert!(d.caret_in_table());
    }

    #[test]
    fn insert_thematic_break_spells_the_rule_the_format_s_own_way() {
        // The whole point of delegating: `---` is Markdown's, `* * *` is djot's,
        // and leaf wrote the first into both until twig started spelling it.
        let mut md = doc_with("hr_md", "para\n");
        md.caret = 2;
        md.insert_thematic_break();
        assert_eq!(md.source, "pa\n\n---\n\nra\n");

        let mut dj = Doc::from_source("para\n".into(), Format::Djot).unwrap();
        dj.caret = 2;
        dj.insert_thematic_break();
        assert_eq!(dj.source, "pa\n\n* * *\n\nra\n");
    }

    #[test]
    fn insert_table_parts_the_paragraph_and_lands_in_the_first_header_cell() {
        // The table goes *at* the caret the way the rule does: the paragraph is
        // parted first, and twig writes the grid after its first half. The
        // caret then sits in the first header cell — selected, as Tab would
        // leave it — so the next keystroke is the heading.
        let mut d = doc_with("table_mid", "before after\n");
        d.caret = 7;
        d.insert_table(2, 3);
        assert_eq!(
            d.source,
            "before \n\n|  |  |  |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |\n\nafter\n"
        );
        assert!(d.caret_in_table());
        let first_bar = d.source.find('|').unwrap();
        assert!(
            d.caret > first_bar && d.caret < d.source.find("| ---").unwrap(),
            "caret {} is not in the header row",
            d.caret
        );
        d.insert("Name");
        assert!(d.source.starts_with("before \n\n| Name |  |  |\n"));
        // And the grid the table was written into is one the table keys walk
        // (over the map a frontend rebuilds after every edit).
        d.build_visual(80);
        assert!(d.cell_tab(true));
        d.insert("Qty");
        assert!(d.source.starts_with("before \n\n| Name | Qty |  |\n"));
    }

    #[test]
    fn insert_table_spells_the_grid_the_format_s_own_way() {
        // Djot's delimiter row is unpadded, and leaf never has to know that.
        let mut dj = Doc::from_source("para\n".into(), Format::Djot).unwrap();
        dj.caret = 2;
        dj.insert_table(1, 2);
        assert_eq!(dj.source, "pa\n\n|  |  |\n|---|---|\n|  |  |\n\nra\n");
        assert!(dj.caret_in_table());
    }

    #[test]
    fn insert_table_refuses_where_the_format_spells_no_table() {
        let mut d = Doc::from_source("<p>ab</p>\n".into(), Format::Html).unwrap();
        d.caret = 4;
        d.insert_table(1, 1);
        assert_eq!(d.source, "<p>ab</p>\n");
        assert!(d.status.as_deref().unwrap_or("").contains("not supported"));
        assert!(!d.capabilities().table);
    }

    #[test]
    fn insert_table_reports_a_zero_shape_and_writes_nothing() {
        let mut d = doc_with("table_zero", "para\n");
        d.caret = 2;
        d.insert_table(0, 2);
        assert_eq!(d.source, "para\n");
        assert!(d.status.as_deref().unwrap_or("").starts_with("table:"));
    }

    #[test]
    fn clicking_below_a_final_thematic_break_can_type_after_it() {
        let mut d = wysiwyg_doc("hr_final_click", "---\n");
        d.build_visual(80);
        d.click(d.vmap.num_rows() + 2, 0, false);
        assert_eq!(d.caret, d.source.len(), "the caret belongs after the rule");
        d.insert("after");
        assert_eq!(d.source, "---\nafter");
    }

    #[test]
    fn enter_in_a_nested_list_item_keeps_the_new_item_nested() {
        // The same bytes are two documents. In Markdown `  - b` is a nested item
        // and the next one belongs beside it, at its indent. In Djot a list
        // marker can't interrupt a paragraph, so those bytes are literal text in
        // item `a` and there is only one item — writing `  - ` under it would add
        // no item at all, just more text, and the new sibling has to go to
        // column zero. Both spellings come out of the *enclosing item's* line.
        let mut md = wysiwyg_doc("enter_nested_md", "- a\n  - b\n");
        md.caret = "- a\n  - b".len();
        md.newline();
        assert_eq!(md.source, "- a\n  - b\n  - \n");
        assert_eq!(list_items(&mut md), 3);

        let mut dj = Doc::from_source("- a\n  - b\n".into(), Format::Djot).unwrap();
        dj.view = View::Wysiwyg;
        dj.build_visual(80);
        dj.caret = "- a\n  - b".len();
        dj.newline();
        assert_eq!(dj.source, "- a\n  - b\n- \n");
        assert_eq!(list_items(&mut dj), 2);

        // Where Djot's nesting is real — opened by a blank line — the indent is
        // reproduced there too, and the two formats agree again.
        let mut dj = Doc::from_source("- a\n\n  - b\n".into(), Format::Djot).unwrap();
        dj.view = View::Wysiwyg;
        dj.build_visual(80);
        dj.caret = "- a\n\n  - b".len();
        dj.newline();
        assert_eq!(dj.source, "- a\n\n  - b\n  - \n");
        assert_eq!(list_items(&mut dj), 3);
    }

    #[test]
    fn tab_nests_an_item_at_the_column_its_own_marker_asks_for() {
        // Tab replaces the line's whole prefix with the one twig spells, so the
        // quote markers, the parent's indent and an ordered marker's extra
        // column are all its answer rather than leaf's arithmetic.
        for (name, body, caret, want) in [
            ("bullet", "- a\n- b\n", 6, "- a\n  - b\n"),
            ("ordered", "1. a\n2. b\n", 8, "1. a\n   1. b\n"),
            ("quoted", "> - a\n> - b\n", 10, "> - a\n>   - b\n"),
            // A checkbox is markup the item's own text wraps past, but a nested
            // list may only open at the *list* marker's column — four in from
            // there is a paragraph continuation, and `- [ ] a\n      - [ ] b`
            // parses as one item, not two.
            ("task", "- [ ] a\n- [ ] b\n", 14, "- [ ] a\n  - [ ] b\n"),
            (
                "quoted task",
                "> - [ ] a\n> - [ ] b\n",
                18,
                "> - [ ] a\n>   - [ ] b\n",
            ),
        ] {
            let mut doc = wysiwyg_doc(name, body);
            doc.caret = caret;
            doc.indent();
            assert_eq!(doc.source, want, "{name}");
            // The nesting is real, not just indented text.
            assert_eq!(list_items(&mut doc), 2, "{name}");
        }
    }

    #[test]
    fn backspace_only_outdents_where_the_format_says_there_is_an_item() {
        // The same bytes, the two formats disagreeing, and a gesture that used
        // to read the bytes. `  - b` is a nested item in Markdown, so Backspace
        // at its marker outdents. In Djot a marker can't interrupt a paragraph,
        // so those bytes are literal text inside item `a` — there is nothing to
        // outdent, and treating them as a marker turned one item into two, a
        // structural edit from a keystroke that should delete one character.
        //
        // twig's `line_prefix` is what tells them apart: it reports the marker
        // on the Markdown line and nothing on the Djot one, which is a
        // continuation. No byte scan can reach that answer.
        let src = "- a\n  - b\n";
        let at = "- a\n  - ".len();

        let mut md = Doc::from_source(src.into(), Format::Markdown).unwrap();
        md.view = View::Wysiwyg;
        md.build_visual(80);
        md.caret = at;
        md.backspace();
        assert_eq!(md.source, "- a\n- b\n");
        assert_eq!(list_items(&mut md), 2);

        let mut dj = Doc::from_source(src.into(), Format::Djot).unwrap();
        dj.view = View::Wysiwyg;
        dj.build_visual(80);
        dj.caret = at;
        dj.backspace();
        assert_eq!(dj.source, "- a\n  -b\n"); // an ordinary character delete
        assert_eq!(list_items(&mut dj), 1); // and the structure is untouched
    }

    #[test]
    fn enter_in_a_checklist_item_starts_another_unchecked_one() {
        // Leaf used to spell the next item from the marker bytes it scanned, and
        // its scanner stopped at the bullet — so Enter in a checklist wrote `- `
        // and dropped out of the checklist. twig reproduces the whole
        // continuation, and a fresh item is always unticked however the one above
        // it stands.
        for (name, body, want) in [
            ("unchecked", "- [ ] a\n", "- [ ] a\n- [ ] \n"),
            ("checked", "- [x] a\n", "- [x] a\n- [ ] \n"),
        ] {
            let mut doc = wysiwyg_doc(name, body);
            doc.caret = body.trim_end_matches('\n').len();
            doc.newline();
            assert_eq!(doc.source, want, "{name}");
            // Both items are checklist items — the new one is a box, not the
            // plain bullet the old marker scan left behind — and it is unticked
            // whichever way the one above it faces.
            let boxes: Vec<Option<bool>> = doc
                .nodes()
                .iter()
                .filter(|n| n.kind == Kind::TaskListItem)
                .map(|n| n.checked)
                .collect();
            assert_eq!(boxes.len(), 2, "{name}");
            assert_eq!(boxes[1], Some(false), "{name}");
        }
    }

    #[test]
    fn a_split_takes_the_space_the_caret_was_in_front_of() {
        // Splicing a break at the caret strands the space the words were parted
        // at on the head of the second block, where it reads as an indent nobody
        // typed. twig's split consumes it.
        for (name, body, caret, want) in [
            ("para", "one two\n", 3, "one\n\ntwo\n"),
            ("item", "- one two\n", 5, "- one\n- two\n"),
            ("quote", "> one two\n", 5, "> one\n>\n> two\n"),
            // A heading takes leaf's own path, which has to match.
            ("heading", "# one two\n", 5, "# one\n\ntwo\n"),
        ] {
            let mut doc = wysiwyg_doc(name, body);
            doc.caret = caret;
            doc.newline();
            assert_eq!(doc.source, want, "{name}");
        }
    }

    #[test]
    fn enter_at_the_end_of_a_heading_opens_a_paragraph() {
        // The one place leaf keeps its own break: `split_block` repeats the `#`,
        // and Enter after a title is how the body under it is asked for.
        let mut doc = wysiwyg_doc("head_enter", "# Title\n");
        doc.caret = "# Title".len();
        doc.newline();
        doc.insert("body");
        assert_eq!(doc.source, "# Title\n\nbody\n");
        assert_eq!(
            doc.nodes()
                .iter()
                .filter(|n| n.kind == Kind::Heading)
                .count(),
            1
        );
    }

    #[test]
    fn enter_in_a_quoted_list_item_starts_the_next_quoted_item() {
        // A quoted item's marker doesn't open its line, so a scan that starts at
        // column zero finds a `>` where it wanted a bullet, calls the line "not a
        // list" and hands Enter to the plain-quote branch — which writes `> ` and
        // drops the list. The next item has to carry the whole prefix.
        for (name, body, want) in [
            ("flat", "> - a\n", "> - a\n> - \n"),
            ("sibling", "> - a\n> - b\n", "> - a\n> - b\n> - \n"),
            ("nested", "> - a\n>   - b\n", "> - a\n>   - b\n>   - \n"),
            ("ordered", "> 1. a\n> 2. b\n", "> 1. a\n> 2. b\n> 3. \n"),
            ("twice quoted", "> > - a\n", "> > - a\n> > - \n"),
        ] {
            let mut doc = wysiwyg_doc(name, body);
            doc.caret = body.trim_end_matches('\n').len();
            doc.newline();
            assert_eq!(doc.source, want, "{name}");
            // The marker isn't just spelled right, it parses as an item.
            assert_eq!(list_items(&mut doc), body.lines().count() + 1, "{name}");
        }
    }

    #[test]
    fn an_empty_quoted_item_leaves_the_list_and_stays_in_the_quote() {
        // Double-Enter exits the list. Unquoted that means a blank line, but a
        // *bare* blank line would end the quote too and drop the caret out of it,
        // so the separator keeps its `>` and the caret's line keeps its `> `.
        let mut doc = wysiwyg_doc("quoted_exit", "> - a\n> - \n");
        doc.caret = "> - a\n> - ".len();
        doc.newline();
        assert_eq!(doc.source, "> - a\n>\n> \n");
        assert_eq!(list_items(&mut doc), 1);
        // What "still in the quote" means for the next keystroke: the caret sits
        // behind the prefix, and what's typed there lands inside the quote as a
        // paragraph of its own — not as more of item `a`.
        doc.insert("x");
        assert_eq!(doc.source, "> - a\n>\n> x\n");
        assert!(
            doc.editor
                .ancestors_at(doc.caret - 1)
                .is_ok_and(|c| c.into_iter().any(|m| m.kind == Kind::BlockQuote))
        );
    }

    #[test]
    fn backspace_at_a_quoted_marker_takes_the_marker_and_leaves_the_quote() {
        // The marker is hidden block markup, so Backspace over it is structural —
        // but only the marker is the list's. Splicing from the line start would
        // take the `>` with it and silently unquote the line.
        let mut doc = wysiwyg_doc("quoted_bksp", "> - a\n");
        doc.caret = "> - ".len();
        doc.backspace();
        assert_eq!(doc.source, "> a\n");
        assert_eq!(list_items(&mut doc), 0);

        // A nested one outdents instead, moving the bullet within the quote
        // rather than moving the quote.
        let mut doc = wysiwyg_doc("quoted_outdent", "> - a\n>   - b\n");
        doc.caret = "> - a\n>   - ".len();
        doc.backspace();
        assert_eq!(doc.source, "> - a\n> - b\n");
        assert_eq!(list_items(&mut doc), 2);
    }

    #[test]
    fn only_a_bare_paragraph_is_parted_around_the_caret() {
        // The split is deliberately narrow. Parting a fenced block would leave
        // two fences with a rule between them, and parting a list item would
        // mint an item nobody asked for on the way to a rule that lands after
        // the list either way — so both keep the whole block intact and take the
        // rule after it. A caret in a quote is likewise left alone.
        for (name, body, caret, want) in [
            (
                "code",
                "```\nfn x() {}\n```\n",
                8,
                "```\nfn x() {}\n```\n\n---\n\n",
            ),
            ("list", "- one two\n", 6, "- one two\n\n---\n\n"),
            ("quote", "> one two\n", 6, "> one two\n>\n> ---\n>\n> \n"),
        ] {
            let mut d = doc_with(&format!("hr_narrow_{name}"), body);
            d.caret = caret;
            d.insert_thematic_break();
            assert_eq!(d.source, want, "{name}: the block should stay whole");
        }
    }

    #[test]
    fn insert_thematic_break_replaces_the_selection() {
        // Now that the rule lands *at* the caret again, replacing the selection
        // is coherent once more: the text goes, and the rule takes its place.
        // The space the deletion left leading the second half is consumed by the
        // split rather than opening the new paragraph with it.
        let mut d = doc_with("hr_sel", "one two three\n");
        d.anchor = Some(4);
        d.caret = 7; // "two"
        d.insert_thematic_break();
        assert_eq!(d.source, "one \n\n---\n\nthree\n");
        assert_eq!(d.selection(), None);
    }

    #[test]
    fn insert_thematic_break_clears_a_code_block_and_a_table_rather_than_refusing() {
        // Both are blocks the rule lands *after*. Leaf used to refuse a fence,
        // because writing `---` into one is code, not a rule — twig now walks out
        // to the block that owns the caret's line, so there is nothing to refuse.
        let mut code = doc_with("hr_code", "```\nfn x() {}\n```\n");
        code.caret = 5; // inside the fenced code
        code.insert_thematic_break();
        assert_eq!(code.source, "```\nfn x() {}\n```\n\n---\n\n");
        assert_eq!(code.status, None, "no refusal to report any more");

        let mut table = doc_with("hr_table", "| a | b |\n|---|---|\n| 1 | 2 |\n");
        table.caret = 3; // in the header row
        table.insert_thematic_break();
        assert_eq!(table.source, "| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n");
    }

    #[test]
    fn insert_thematic_break_in_a_list_item_ends_the_list() {
        // The un-indented rule cannot continue the list, so it closes the list
        // and lands at the top level rather than nested inside it.
        let mut d = doc_with("hr_list", "- one\n- two\n");
        d.caret = "- one\n- tw".len(); // mid "two"
        d.insert_thematic_break();
        d.build_visual(80);
        let rule_at = d.source.find("---").unwrap();
        assert_eq!(kind_at(&mut d, rule_at), Some(Kind::ThematicBreak));
        assert!(
            !d.nodes().iter().any(|n| n.kind == Kind::BulletList
                && n.span.start <= rule_at
                && rule_at < n.span.end),
            "the rule must not be nested inside the list"
        );
    }

    #[test]
    fn insert_thematic_break_in_a_blockquote_stays_in_the_quote() {
        // Leaf used to end the quote. twig gives the rule the quote's own prefix,
        // which is the document the gesture was actually asked for — and the
        // line the caret goes on to under it wears the prefix too, as the line
        // Enter opens in a quote does.
        let mut d = doc_with("hr_quote", "> hello\n");
        d.caret = 4; // inside the quoted text
        d.insert_thematic_break();
        assert_eq!(d.source, "> hello\n>\n> ---\n>\n> \n");
        assert_eq!(d.caret, "> hello\n>\n> ---\n>\n> ".len());
        d.build_visual(80);
        let rule_at = d.source.find("---").unwrap();
        assert_eq!(kind_at(&mut d, rule_at), Some(Kind::ThematicBreak));
        assert!(
            d.nodes().iter().any(|n| n.kind == Kind::BlockQuote
                && n.span.start <= rule_at
                && rule_at < n.span.end),
            "the rule belongs to the quote it was asked for"
        );
    }

    // ── typing against a block picture ────────────────────────────────────────

    /// A rendered-view document with the caret parked on one of the picture's two
    /// stops, and the map already built — the state a frontend is in between
    /// drawing a frame and the next keystroke.
    fn doc_at_picture(name: &str, src: &str, side: MediaStop) -> Doc {
        let mut d = doc_in(View::Wysiwyg, name, src);
        d.build_visual_unwrapped();
        let start = src.find("![").unwrap();
        d.caret = match side {
            MediaStop::Before => start,
            MediaStop::After => start + "![](p.png)".len(),
        };
        d
    }

    /// The block media the map publishes, after rebuilding it — "is this still a
    /// picture, or has it become a line of text with an image in it?"
    fn media_count(d: &mut Doc) -> usize {
        d.build_visual_unwrapped();
        d.vmap.media.len()
    }

    #[test]
    fn typing_past_a_block_picture_opens_a_paragraph_under_it() {
        // The accident this prevents: tap the blank page under a photo (which
        // lands on the picture's trailing stop), type, and `![](p.png)xy` is a
        // paragraph with an *inline* image — the photo stops being drawn.
        let mut d = doc_at_picture("pic_after", "hi\n\n![](p.png)\n", MediaStop::After);
        d.insert("xy");
        assert_eq!(d.source, "hi\n\n![](p.png)\n\nxy\n");
        assert_eq!(media_count(&mut d), 1, "still a picture");
    }

    #[test]
    fn typing_in_front_of_a_block_picture_opens_a_paragraph_above_it() {
        let mut d = doc_at_picture("pic_before", "hi\n\n![](p.png)\n", MediaStop::Before);
        d.insert("xy");
        assert_eq!(d.source, "hi\n\nxy\n\n![](p.png)\n");
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn a_picture_that_opens_the_document_still_takes_a_paragraph_above_it() {
        let mut d = doc_at_picture("pic_first", "![](p.png)\n", MediaStop::Before);
        d.insert("x");
        assert_eq!(d.source, "x\n\n![](p.png)\n");
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn one_undo_puts_the_picture_back_the_way_it_was_found() {
        // The opened paragraph is part of the keystroke, not an edit the writer
        // made — so it undoes with the character, not a step later.
        let mut d = doc_at_picture("pic_undo", "hi\n\n![](p.png)\n", MediaStop::After);
        d.insert("x");
        assert_eq!(d.source, "hi\n\n![](p.png)\n\nx\n");
        d.undo();
        assert_eq!(d.source, "hi\n\n![](p.png)\n");
    }

    #[test]
    fn pasting_against_a_block_picture_opens_a_paragraph_too() {
        // ⌘V dissolves the picture exactly as a keystroke does.
        let mut d = doc_at_picture("pic_paste", "hi\n\n![](p.png)\n", MediaStop::After);
        d.paste("pasted");
        assert_eq!(d.source, "hi\n\n![](p.png)\n\npasted\n");
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn typing_beside_an_inline_image_is_ordinary_editing() {
        // An inline image has no placeholder row and no stops of its own. Opening
        // a paragraph mid-sentence would be the bug, not the fix.
        let mut d = doc_in(View::Wysiwyg, "pic_inline", "see ![](p.png) here\n");
        d.build_visual_unwrapped();
        d.caret = "see ![](p.png)".len();
        d.insert("!");
        assert_eq!(d.source, "see ![](p.png)! here\n");
    }

    #[test]
    fn source_view_types_raw_markup_against_an_image_untouched() {
        // Source view is for writing the markup itself; a break inserted behind
        // the writer's back there would be the editor arguing with them.
        let mut d = doc_in(View::Source, "pic_src", "![](p.png)\n");
        d.caret = "![](p.png)".len();
        d.insert("x");
        assert_eq!(d.source, "![](p.png)x\n");
    }

    #[test]
    fn typing_over_a_selection_that_starts_at_a_picture_stop_replaces_it() {
        // A selection is replaced, not joined into, so there is nothing to
        // protect: the range takes the picture with it.
        let mut d = doc_at_picture("pic_sel", "hi\n\n![](p.png)\n", MediaStop::Before);
        d.anchor = Some(d.caret);
        d.caret = d.source.find("![").unwrap() + "![](p.png)".len();
        d.insert("x");
        assert_eq!(d.source, "hi\n\nx\n");
    }

    #[test]
    fn backspace_past_a_block_picture_deletes_the_picture_not_its_last_byte() {
        // What this actually cost: a real vault's photo, to one stray Backspace.
        // The caret past `![](p.png)` was deleting the closing paren — invisible
        // in the rendered view — and the photo became the text `![](p.png`.
        let mut d = doc_at_picture("pic_bs", "hi\n\n![](p.png)\n", MediaStop::After);
        d.backspace();
        assert_eq!(d.source, "hi\n");
        assert_eq!(media_count(&mut d), 0, "the picture went, in one piece");
        d.undo();
        assert_eq!(
            d.source, "hi\n\n![](p.png)\n",
            "and comes back in one piece"
        );
    }

    #[test]
    fn backspace_in_front_of_a_block_picture_steps_out_instead_of_merging_it() {
        // Deleting the break here would join the picture to the paragraph above,
        // where it is an *inline* image and stops being drawn. Step over the
        // boundary; the next press deletes in the paragraph the caret reached.
        let mut d = doc_at_picture("pic_bs_before", "hi\n\n![](p.png)\n", MediaStop::Before);
        d.backspace();
        assert_eq!(d.source, "hi\n\n![](p.png)\n", "nothing deleted");
        assert_eq!(d.caret, 2, "the caret stepped up to the end of `hi`");
        d.backspace();
        assert_eq!(d.source, "h\n\n![](p.png)\n", "and now it deletes there");
        assert_eq!(media_count(&mut d), 1, "the picture was never at risk");
    }

    #[test]
    fn forward_delete_in_front_of_a_block_picture_deletes_the_picture() {
        // The mirror. A byte-step here eats the `!` and leaves a link.
        let mut d = doc_at_picture("pic_del", "hi\n\n![](p.png)\n\nbye\n", MediaStop::Before);
        d.delete_forward();
        assert_eq!(d.source, "hi\n\nbye\n");
        assert_eq!(media_count(&mut d), 0);
    }

    #[test]
    fn forward_delete_past_a_block_picture_steps_over_the_boundary() {
        let mut d = doc_at_picture(
            "pic_del_after",
            "hi\n\n![](p.png)\n\nbye\n",
            MediaStop::After,
        );
        d.delete_forward();
        assert_eq!(d.source, "hi\n\n![](p.png)\n\nbye\n", "nothing deleted");
        assert_eq!(
            d.caret,
            d.source.find("bye").unwrap(),
            "the caret stepped down to `bye`"
        );
    }

    #[test]
    fn a_picture_that_is_the_whole_document_still_deletes_cleanly() {
        let mut d = doc_at_picture("pic_only", "![](p.png)\n", MediaStop::After);
        d.backspace();
        assert_eq!(d.source, "\n");
        assert_eq!(media_count(&mut d), 0);
    }

    #[test]
    fn a_word_delete_takes_the_picture_whole_or_steps_out_of_it() {
        // ⌥⌫ past a picture would otherwise eat a "word" of its markup.
        let mut d = doc_at_picture("pic_wordbs", "hi there\n\n![](p.png)\n", MediaStop::After);
        d.delete_word_back();
        assert_eq!(d.source, "hi there\n");

        // And in front of one it runs *through* the paragraph break into the
        // prose above, which merges the picture inline — so it steps out first,
        // and the second press deletes the word it was aimed at.
        let mut d = doc_at_picture("pic_wordbs2", "hi there\n\n![](p.png)\n", MediaStop::Before);
        d.delete_word_back();
        assert_eq!(d.source, "hi there\n\n![](p.png)\n");
        d.delete_word_back();
        assert_eq!(
            d.source, "hi \n\n![](p.png)\n",
            "the word above went, the picture stayed"
        );
        assert_eq!(media_count(&mut d), 1);
    }

    #[test]
    fn source_view_deletes_raw_markup_against_an_image_untouched() {
        let mut d = doc_in(View::Source, "pic_src_del", "![](p.png)\n");
        d.caret = "![](p.png)".len();
        d.backspace();
        assert_eq!(d.source, "![](p.png\n", "raw editing, byte by byte");
    }

    // ── typing against a block leaf directive ─────────────────────────────────

    /// A rendered-view `fmt` document with the caret on one of the stops of
    /// the directive spelled `mark`, and the map built — [`doc_at_picture`]
    /// for a directive, in either format.
    fn doc_at_directive(src: &str, fmt: Format, mark: &str, side: MediaStop) -> Doc {
        let mut d = Doc::from_source(src.into(), fmt).unwrap();
        d.view = View::Wysiwyg;
        d.build_visual_unwrapped();
        let start = src.find(mark).unwrap();
        d.caret = match side {
            MediaStop::Before => start,
            MediaStop::After => start + mark.len(),
        };
        assert!(d.vmap.is_stop(d.caret), "{src:?} {side:?}");
        d
    }

    /// The leaf directives the map publishes, by name, after rebuilding it —
    /// "is this still a directive, or has it become a paragraph of source?"
    fn directive_names(d: &mut Doc) -> Vec<String> {
        d.build_visual_unwrapped();
        d.vmap.directives.iter().map(|i| i.name.clone()).collect()
    }

    /// Markdown's `::embed{…}` and djot's empty fence, each with the source
    /// typing at its before and after stops should leave.
    const DIRECTIVE_CASES: [(Format, &str, &str, &str, &str); 3] = [
        (
            Format::Markdown,
            "hi\n\n::embed{src=x}\n",
            "::embed{src=x}",
            "hi\n\nB\n\n::embed{src=x}\n",
            "hi\n\n::embed{src=x}\n\nB\n",
        ),
        (
            Format::Markdown,
            "hi\n\n::page-break\n\nbye\n",
            "::page-break",
            "hi\n\nB\n\n::page-break\n\nbye\n",
            "hi\n\n::page-break\n\nB\n\nbye\n",
        ),
        (
            Format::Djot,
            "hi\n\n::: page-break\n:::\n\nbye\n",
            "::: page-break\n:::",
            "hi\n\nB\n\n::: page-break\n:::\n\nbye\n",
            "hi\n\n::: page-break\n:::\n\nB\n\nbye\n",
        ),
    ];

    #[test]
    fn typing_past_a_leaf_directive_opens_a_paragraph_under_it() {
        // The task's accident: `::page[…]{…}TWO` is a paragraph of raw source.
        for (fmt, src, mark, _, after) in DIRECTIVE_CASES {
            let mut d = doc_at_directive(src, fmt, mark, MediaStop::After);
            let names = directive_names(&mut d);
            d.insert("B");
            assert_eq!(d.source, after, "{fmt:?}");
            assert_eq!(directive_names(&mut d), names, "{fmt:?}: still a directive");
        }
    }

    #[test]
    fn typing_in_front_of_a_leaf_directive_opens_a_paragraph_above_it() {
        // Found in leaf-tui: `B` typed here made `B::embed{src=…}`.
        for (fmt, src, mark, before, _) in DIRECTIVE_CASES {
            let mut d = doc_at_directive(src, fmt, mark, MediaStop::Before);
            let names = directive_names(&mut d);
            d.insert("B");
            assert_eq!(d.source, before, "{fmt:?}");
            assert_eq!(directive_names(&mut d), names, "{fmt:?}: still a directive");
        }
        // And one that opens the document takes a paragraph above it too.
        let mut d = doc_at_directive(
            "::embed{src=x}\n",
            Format::Markdown,
            "::",
            MediaStop::Before,
        );
        d.insert("B");
        assert_eq!(d.source, "B\n\n::embed{src=x}\n");
        assert_eq!(directive_names(&mut d), ["embed"]);
    }

    #[test]
    fn two_directives_in_a_row_take_a_paragraph_between_them_from_either_side() {
        // The task's document: no paragraph between the marks to click into, so
        // the end of the first mark's row, or the front of the second's, is the
        // only way in.
        let md = "::page[Page 1 of 2]{card=\"a\"}\n\n::page[Page 2 of 2]{card=\"b\"}\n";
        let md_want = "::page[Page 1 of 2]{card=\"a\"}\n\nTWO\n\n::page[Page 2 of 2]{card=\"b\"}\n";
        let dj = "::: page-break\n:::\n\n::: x-card\n:::\n";
        let dj_want = "::: page-break\n:::\n\nTWO\n\n::: x-card\n:::\n";
        for (fmt, src, first, second, want) in [
            (
                Format::Markdown,
                md,
                "::page[Page 1 of 2]{card=\"a\"}",
                "::page[Page 2",
                md_want,
            ),
            (
                Format::Djot,
                dj,
                "::: page-break\n:::",
                "::: x-card",
                dj_want,
            ),
        ] {
            for (mark, side) in [(first, MediaStop::After), (second, MediaStop::Before)] {
                let mut d = doc_at_directive(src, fmt, mark, side);
                let names = directive_names(&mut d);
                assert_eq!(names.len(), 2);
                d.insert("T");
                d.insert("W");
                d.insert("O");
                assert_eq!(d.source, want, "{fmt:?} {side:?}");
                assert_eq!(directive_names(&mut d), names, "{fmt:?} {side:?}");
                let labels: Vec<_> = d.vmap.directives.iter().map(|i| i.label.clone()).collect();
                if fmt == Format::Markdown {
                    assert_eq!(labels, ["Page 1 of 2", "Page 2 of 2"]);
                    assert_eq!(d.vmap.directives[1].attr("card"), Some("b"));
                }
            }
        }
    }

    #[test]
    fn a_djot_directive_s_attribute_line_stays_with_it() {
        // The `{…}` line above the fence is the directive's markup: a paragraph
        // opened between the two would take the attributes, and a delete that
        // left the line would hand them to the paragraph below.
        let src = "hi\n\n{.wide src=\"x\"}\n::: x-card\n:::\n\nbye\n";
        let mark = "::: x-card\n:::";
        let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::Before);
        d.insert("B");
        assert_eq!(
            d.source,
            "hi\n\nB\n\n{.wide src=\"x\"}\n::: x-card\n:::\n\nbye\n"
        );
        assert_eq!(directive_names(&mut d), ["x-card"]);
        assert_eq!(d.vmap.directives[0].attr("src"), Some("x"));

        let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::After);
        d.backspace();
        assert_eq!(
            d.source, "hi\n\nbye\n",
            "the directive went with its attributes"
        );
        let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::Before);
        d.delete_forward();
        assert_eq!(d.source, "hi\n\nbye\n");

        // A djot block picture's attribute line is the same markup, kept the
        // same way.
        let pic = "hi\n\n{.wide}\n![](p.png)\n";
        let mut d = doc_at_directive(pic, Format::Djot, "![](p.png)", MediaStop::Before);
        d.insert("B");
        assert_eq!(d.source, "hi\n\nB\n\n{.wide}\n![](p.png)\n");
        assert_eq!(media_count(&mut d), 1);
        let mut d = doc_at_directive(pic, Format::Djot, "![](p.png)", MediaStop::After);
        d.backspace();
        assert_eq!(d.source, "hi\n");

        // Where the attribute line opens the document, too.
        let src = "{.wide}\n::: x-card\n:::\n";
        let mut d = doc_at_directive(src, Format::Djot, mark, MediaStop::Before);
        d.insert("B");
        assert_eq!(d.source, "B\n\n{.wide}\n::: x-card\n:::\n");
        assert_eq!(directive_names(&mut d), ["x-card"]);
    }

    #[test]
    fn a_directive_drawn_taller_still_takes_a_paragraph_under_it() {
        // Reserved filler rows are decoration: the after-stop is still the
        // directive's end, and a click on the drawing's lower part lands there.
        let src = "hi\n\n::embed{src=x}\n";
        let mut d = doc_at_directive(src, Format::Markdown, "::embed{src=x}", MediaStop::After);
        let key = d.vmap.directives[0].key();
        d.set_directive_rows(HashMap::from([(key, 4)]));
        d.build_visual_unwrapped();
        assert_eq!(d.vmap.directives[0].rows_span.len(), 4);
        d.insert("B");
        assert_eq!(d.source, "hi\n\n::embed{src=x}\n\nB\n");
    }

    #[test]
    fn pasting_against_a_leaf_directive_opens_a_paragraph_too() {
        for (fmt, src, mark, before, after) in DIRECTIVE_CASES {
            for (side, want) in [(MediaStop::Before, before), (MediaStop::After, after)] {
                let mut d = doc_at_directive(src, fmt, mark, side);
                d.paste("pasted");
                assert_eq!(d.source, want.replace("B", "pasted"), "{fmt:?} {side:?}");
            }
        }
    }

    #[test]
    fn one_undo_puts_a_directive_back_the_way_it_was_found() {
        for (fmt, src, mark, ..) in DIRECTIVE_CASES {
            for side in [MediaStop::Before, MediaStop::After] {
                let mut d = doc_at_directive(src, fmt, mark, side);
                d.insert("x");
                d.insert("y");
                assert_ne!(d.source, src);
                d.undo();
                assert_eq!(d.source, src, "{fmt:?} {side:?}");
            }
        }
    }

    #[test]
    fn delete_keys_take_a_leaf_directive_whole_or_step_out_of_it() {
        // The picture's rule: the key aimed at the directive takes it whole, the
        // key aimed away steps over the boundary — never a byte of its markup.
        for (fmt, src, mark, ..) in DIRECTIVE_CASES {
            let gone = src
                .replace(&format!("{mark}\n\n"), "")
                .replace(&format!("\n\n{mark}"), "");
            let mut d = doc_at_directive(src, fmt, mark, MediaStop::After);
            d.backspace();
            assert_eq!(d.source, gone, "{fmt:?}: Backspace past it");
            d.undo();
            assert_eq!(d.source, src, "{fmt:?}: and back in one piece");

            let mut d = doc_at_directive(src, fmt, mark, MediaStop::Before);
            d.delete_forward();
            assert_eq!(d.source, gone, "{fmt:?}: Delete in front of it");

            let mut d = doc_at_directive(src, fmt, mark, MediaStop::Before);
            d.backspace();
            assert_eq!(d.source, src, "{fmt:?}: Backspace in front deletes nothing");
            assert_eq!(d.caret, 2, "{fmt:?}: it steps up to the end of `hi`");

            let mut d = doc_at_directive(src, fmt, mark, MediaStop::After);
            let at = d.caret;
            d.delete_forward();
            assert_eq!(d.source, src, "{fmt:?}: Delete past it deletes nothing");
            assert!(
                d.caret > at || src.ends_with(&format!("{mark}\n")),
                "{fmt:?}"
            );
        }
    }

    #[test]
    fn source_view_types_raw_markup_against_a_directive_untouched() {
        let mut d = doc_in(View::Source, "dir_src", "::embed{src=x}\n");
        d.caret = "::embed{src=x}".len();
        d.insert("x");
        assert_eq!(d.source, "::embed{src=x}x\n");
    }

    #[test]
    fn image_destination_at_caret_reads_the_image_under_the_caret() {
        let mut d = doc_with("img_read", "![a cat](cat.png)\n");
        d.caret = 3; // inside the image markup
        assert_eq!(d.image_destination_at_caret(), Some("cat.png".to_string()));
        // Past the image, the caret is in no image.
        d.caret = "![a cat](cat.png)".len();
        assert_eq!(d.image_destination_at_caret(), None);
    }

    #[test]
    fn set_media_rows_reserves_blank_filler_rows_the_frontend_paints_over() {
        // The image is one placeholder row by default, and `set_media_rows` grows
        // it to the height the frontend measured: the label row plus blank
        // `decoration` fillers that hold the vertical space a raster is drawn into.
        let mut d = wysiwyg_doc("img_rows", "intro\n\n![a cat](cat.png)\n\nend\n");
        assert_eq!(d.vmap.media.len(), 1);
        let img_row = d.vmap.media[0].rows_span.start;
        assert_eq!(
            d.vmap.media[0].rows_span,
            img_row..img_row + 1,
            "default is one row"
        );

        d.set_media_rows(HashMap::from([("cat.png".to_string(), 4)]));
        d.build_visual(80);
        assert_eq!(d.vmap.media.len(), 1, "still one image, now taller");
        let span = d.vmap.media[0].rows_span.clone();
        assert_eq!(span.end - span.start, 4, "reserves the four rows asked for");
        // The label row carries the mark and its glyphs; the three below are blank
        // decoration — drawn, but no caret and no text.
        assert!(
            d.vmap.rows[span.start].media.is_some(),
            "mark rides the first row"
        );
        for r in (span.start + 1)..span.end {
            assert!(d.vmap.rows[r].decoration, "filler row {r} is decoration");
            assert!(d.vmap.rows[r].glyphs.is_empty(), "filler row {r} is blank");
            assert!(
                d.vmap.rows[r].media.is_none(),
                "only the first row is marked"
            );
        }
    }

    #[test]
    fn a_taller_image_adds_no_caret_stops_and_motion_steps_over_its_fillers() {
        // The extra rows are pure spacers: the caret's only homes stay the stop in
        // front of the image and the one just past it, so walking the document top
        // to bottom visits the same offsets whether the image is 1 row or 5.
        let body = "ab\n\n![x](p.png)\n\ncd\n";
        let stops_at = |rows: usize| -> Vec<usize> {
            let mut d = wysiwyg_doc("img_stops", body);
            if rows > 1 {
                d.set_media_rows(HashMap::from([("p.png".to_string(), rows)]));
                d.build_visual(80);
            }
            d.caret = 0;
            let mut seen = vec![d.caret];
            loop {
                d.move_right(false);
                if *seen.last().unwrap() == d.caret {
                    break;
                }
                seen.push(d.caret);
            }
            seen
        };
        assert_eq!(
            stops_at(1),
            stops_at(5),
            "reserving rows must not add stops"
        );
    }

    #[test]
    fn insert_link_repoints_the_link_at_a_bare_caret() {
        let mut d = doc_with("link_repoint", "[word](http://x.dev)\n");
        d.caret = 3; // in the link's text, nothing selected
        d.insert_link("http://y.dev");
        assert_eq!(d.source, "[word](http://y.dev)\n");
        assert_eq!(d.selected_text(), Some("word"));
    }

    #[test]
    fn insert_link_on_an_empty_range_autolinks_a_url() {
        // A link with no text of its own is an autolink, and twig spells it —
        // `<…>` is the canonical form and needs no text typed into it, so the
        // caret lands after it rather than selecting a finished link.
        let mut d = doc_with("link_empty", "\n");
        d.caret = 0;
        d.insert_link("http://x.dev");
        assert_eq!(d.source, "<http://x.dev>\n");
        assert_eq!(d.selection(), None);
        assert_eq!(d.caret, 14);
    }

    #[test]
    fn insert_link_on_an_empty_range_falls_back_for_a_non_url() {
        // `<./notes.md>` is literal text in both formats and `<foo>` is raw HTML
        // in Markdown, so a destination that can't autolink doubles as the text
        // instead — which is then selected, ready to be typed over.
        let mut d = doc_with("link_rel", "\n");
        d.caret = 0;
        d.insert_link("./notes.md");
        assert_eq!(d.source, "[./notes.md](./notes.md)\n");
        assert_eq!(d.selection(), Some((1, 11)));
        d.insert("Notes");
        assert_eq!(d.source, "[Notes](./notes.md)\n");
    }

    #[test]
    fn insert_link_repoints_the_autolink_the_caret_stands_in() {
        // The autolink's text is its URL, so re-pointing replaces the whole
        // node — the caret must not splice a second link inside the first.
        let mut d = doc_with("link_repoint_auto", "see <https://x.dev> ok\n");
        d.caret = 10;
        d.insert_link("https://y.dev");
        assert_eq!(d.source, "see <https://y.dev> ok\n");
    }

    #[test]
    fn code_language_reads_and_edits_through_the_fence() {
        let mut d = doc_with("code_lang", "```rust\nlet x = 1;\n```\n");
        d.caret = 10; // inside the code body
        assert_eq!(d.code_language_at_caret().as_deref(), Some("rust"));
        assert!(d.caret_in_fenced_code());

        d.set_code_language("python");
        assert!(
            d.source.starts_with("```python\n"),
            "source: {:?}",
            d.source
        );
        assert_eq!(d.code_language_at_caret().as_deref(), Some("python"));

        // Clearing it leaves a bare fence and no label.
        d.set_code_language("");
        assert!(d.source.starts_with("```\n"), "source: {:?}", d.source);
        assert_eq!(d.code_language_at_caret(), None);

        // A caret outside any code block edits nothing.
        let mut p = doc_with("code_lang_none", "just prose\n");
        assert!(!p.caret_in_fenced_code());
        p.set_code_language("rust");
        assert_eq!(p.source, "just prose\n");
    }

    #[test]
    fn code_language_reads_and_edits_through_a_quoted_fence() {
        let mut d = doc_with("code_lang_quote", "> ```rust\n> let x = 1;\n> ```\n");
        d.caret = d.source.find("let").unwrap();
        assert_eq!(d.code_language_at_caret().as_deref(), Some("rust"));
        assert!(d.caret_in_fenced_code());
        d.set_code_language("python");
        assert!(
            d.source.starts_with("> ```python\n"),
            "source: {:?}",
            d.source
        );
        assert_eq!(d.code_language_at_caret().as_deref(), Some("python"));
    }

    #[test]
    fn a_language_the_fence_cannot_carry_is_refused_not_written() {
        // Markdown's info string ends at whitespace, so `two words` would write
        // a fence that reads back with a different language than the one asked
        // for. twig refuses it; leaf reports that and leaves the source alone.
        // The old splice trimmed the ends and wrote whatever was left.
        let mut d = doc_with("code_lang_bad", "```rust\nx\n```\n");
        d.caret = 10;
        d.set_code_language("two words");
        assert_eq!(d.source, "```rust\nx\n```\n", "source should be untouched");
        assert!(d.status.is_some(), "the refusal should be reported");
        assert_eq!(d.code_language_at_caret().as_deref(), Some("rust"));
    }

    #[test]
    fn link_destination_at_caret_reads_both_spellings() {
        let mut d = doc_with("link_dest", "see [t](https://x.dev) ok\n");
        d.caret = 5;
        assert_eq!(
            d.link_destination_at_caret().as_deref(),
            Some("https://x.dev")
        );
        d.caret = 0;
        assert_eq!(d.link_destination_at_caret(), None);

        // An autolink has no `destination`; its text is the URL.
        let mut a = doc_with("link_dest_auto", "see <https://x.dev> ok\n");
        a.caret = 10;
        assert_eq!(
            a.link_destination_at_caret().as_deref(),
            Some("https://x.dev")
        );
        a.caret = 21;
        assert_eq!(a.link_destination_at_caret(), None);
    }

    #[test]
    fn heading_at_caret_is_the_nearest_heading_above() {
        let src = "intro\n\n# Title\n\n## The *Second* Part\n\nbody here\n";
        let mut d = doc_with("heading_at", src);
        // Under no heading at all.
        d.caret = 2;
        assert_eq!(d.heading_at_caret(), None);
        // In a paragraph under a level-2 heading: that heading, its markup
        // stripped for the slug, and its own span.
        d.caret = src.find("body").unwrap() + 2;
        let h = d.heading_at_caret().expect("under `## The Second Part`");
        assert_eq!(h.text, "The Second Part");
        assert_eq!(h.level, 2);
        assert_eq!(&src[h.span.clone()].trim_end(), &"## The *Second* Part");
        // Standing in a heading answers with that heading.
        let h = d.heading_at(src.find("Title").unwrap()).unwrap();
        assert_eq!((h.text.as_str(), h.level), ("Title", 1));
        // And the text is what `locate` lands a slug of.
        let landing = d.locate("the-second-part").unwrap();
        assert_eq!(landing.start, d.heading_at_caret().unwrap().span.start);
    }

    #[test]
    fn locate_finds_the_block_a_declared_id_names() {
        // The Book of Mormon shape: one document per chapter, one `{#v…}` per
        // verse. The locator has to land on the *verse*, which is the whole
        // reason a link carries one.
        let src = "{#v1}\nI, Nephi, having been born of goodly parents.\n\n\
                   {#v2}\nYea, I make a record in the language of my father.\n";
        let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
        let v2 = d.locate("v2").expect("the document declares `{#v2}`");
        assert_eq!(
            d.source[v2.start..v2.end].trim_end(),
            "Yea, I make a record in the language of my father."
        );
        // The attribute line is not part of it: `start` is a place to put a
        // caret, and `{#v2}` is markup the caret has no business landing in.
        assert!(d.source[..v2.start].ends_with("{#v2}\n"));
        assert_eq!(d.locate("v99"), None);
    }

    #[test]
    fn locate_reads_a_heading_by_its_words_when_the_format_mints_no_ids() {
        // Markdown has no ids at all — twig mints none, and `{#custom}` in a
        // Markdown heading is literal text. So `#the-second-part` can only be
        // the heading's own words, which is the rule every Markdown renderer
        // already follows and therefore the one a link was authored against.
        let src = "# Title\n\nintro\n\n## The Second Part\n\nbody\n\n## Third\n\nmore\n";
        let mut d = doc_with("locate_md", src);
        let hit = d.locate("the-second-part").expect("the heading's slug");
        assert!(d.source[hit.start..].starts_with("## The Second Part"));
        // Bounded by the next heading that isn't under it, so a peek shows the
        // section rather than only its title.
        assert_eq!(
            &d.source[hit.start..hit.end],
            "## The Second Part\n\nbody\n\n"
        );

        // A subsection does not end its parent: `# Title` runs to `## Third`'s
        // sibling only because there is no other `#`, so it covers the lot.
        let title = d.locate("title").expect("the top heading");
        assert_eq!(title.end, d.source.len());
    }

    #[test]
    fn locate_reads_a_djot_auto_id_however_the_link_spelled_it() {
        // djot mints `Some-Heading-Here`; a link to it is written
        // `#some-heading-here` by nearly everything that writes links. Both
        // spellings are one question.
        let src = "## Some Heading Here\n\nbody\n";
        let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
        let exact = d.locate("Some-Heading-Here").expect("djot's own spelling");
        let slugged = d.locate("some-heading-here").expect("the link's spelling");
        assert_eq!(exact, slugged);
        // The section, not the heading line — there is more to show than a title.
        assert_eq!(&d.source[exact.start..exact.end], src);
    }

    #[test]
    fn locate_ignores_an_empty_locator_and_one_that_slugs_to_nothing() {
        let mut d = doc_with("locate_empty", "# Title\n\nbody\n");
        assert_eq!(d.locate(""), None);
        assert_eq!(d.locate("   "), None);
        // All punctuation: it names nothing, and must not be read as "match the
        // first heading whose slug is also empty".
        assert_eq!(d.locate("!!!"), None);
    }

    #[test]
    fn following_a_fragment_puts_the_caret_on_the_heading_not_before_it() {
        // `landing.start` is the heading's first byte, the `#` of a hidden
        // `## `. The paragraph's end is two bytes back and the heading's `E`
        // three forward, so snapping to the *nearest* stop put the reader at
        // the end of the paragraph above the place they were sent to.
        let src = "see [below](#end) now\n\n## End\n\nlast\n";
        let mut d = doc_in(View::Wysiwyg, "locate_lands", src);
        let landing = d.locate("end").expect("the heading's slug");
        d.place_caret(landing.start, false);
        assert_eq!(d.caret, src.find("End").unwrap());

        // A list item's marker opens its line the same way.
        let src = "intro\n\n- item\n";
        let mut d = doc_in(View::Wysiwyg, "locate_lands_item", src);
        d.place_caret(src.find("- item").unwrap(), false);
        assert_eq!(d.caret, src.find("item").unwrap());

        // The blank line between blocks opens nothing, so a click there still
        // goes to the nearer stop.
        let src = "foo\n\nbar\n";
        let mut d = doc_in(View::Wysiwyg, "locate_lands_gap", src);
        d.place_caret(4, false);
        assert_eq!(d.caret, 3);
    }

    #[test]
    fn locate_gives_a_duplicated_id_to_the_first_block_that_claims_it() {
        // The document's mistake, and the answer every other anchor
        // implementation gives — the alternative is for a link to mean whichever
        // of the two a walk happened to reach first.
        let src = "{#dup}\nfirst.\n\n{#dup}\nsecond.\n";
        let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
        let hit = d.locate("dup").expect("the first `{#dup}`");
        assert_eq!(d.source[hit.start..hit.end].trim_end(), "first.");
    }

    #[test]
    fn insert_footnote_writes_both_halves_and_lands_the_caret_in_the_note() {
        // The button's whole job: a reference where the caret was, a definition
        // to give it meaning, and the caret waiting in the empty note so the
        // next keystroke is the note's first word.
        let mut d = doc_with("fn_insert", "A claim and more.\n");
        d.caret = 7; // just past "A claim"
        d.insert_footnote();
        assert!(
            d.source.starts_with("A claim[^1] and more."),
            "{:?}",
            d.source
        );
        assert!(
            d.source.contains("[^1]:"),
            "the definition too: {:?}",
            d.source
        );
        assert_eq!(d.status, None);

        let reference = d.source.find("[^1]").unwrap();
        let note = d
            .footnote_at(reference + 2)
            .expect("the reference just written");
        assert_eq!(note.label, "1");
        assert_eq!(note.text.as_deref(), Some(""), "the note starts empty");
        assert_eq!(Some(d.caret), note.offset, "the caret waits in the note");
        // …and typing there is typing into the note, not near it.
        d.insert("the note");
        assert_eq!(
            d.footnote_at(reference + 2).and_then(|f| f.text),
            Some("the note".to_string())
        );
    }

    #[test]
    fn insert_footnote_numbers_past_the_notes_already_written() {
        // A second press must not hand back a label somebody else is using: twig
        // reuses a defined label rather than appending a rival definition, so a
        // repeat of `1` would quietly point the new reference at the old note.
        let mut d = doc_with("fn_insert_number", "One[^1] two.\n\n[^1]: first\n");
        d.caret = 7; // past `[^1]`, before " two."
        d.insert_footnote();
        assert!(d.source.starts_with("One[^1][^2] two."), "{:?}", d.source);
        assert_eq!(d.source.matches("[^2]:").count(), 1);
    }

    #[test]
    fn insert_footnote_counts_a_dangling_reference_and_ignores_a_named_one() {
        // `[^2]` with no definition is still a 2 that means something to whoever
        // wrote it — stepping over it would mint a note for their reference. A
        // word label takes no number, so it blocks none.
        let mut d = doc_with("fn_insert_dangling", "a[^2] b[^why] c\n\n[^why]: named\n");
        d.caret = d.source.find(" c").unwrap();
        d.insert_footnote();
        assert!(d.source.contains("[^1]:"), "1 is free: {:?}", d.source);
        assert!(
            d.source.starts_with("a[^2] b[^why][^1] c"),
            "{:?}",
            d.source
        );
    }

    #[test]
    fn insert_footnote_marks_the_selection_rather_than_replacing_it() {
        // A reference annotates the words before it. Consuming the selection —
        // which is what an insert normally does — would delete the very claim
        // the author selected in order to footnote.
        let mut d = doc_with("fn_insert_sel", "A claim and more.\n");
        d.anchor = Some(2);
        d.caret = 7; // "claim" selected
        d.insert_footnote();
        assert!(
            d.source.starts_with("A claim[^1] and more."),
            "{:?}",
            d.source
        );
    }

    #[test]
    fn a_note_just_written_still_knows_where_its_reference_is() {
        // The authoring loop in one test: press the button, type the note, ask to
        // go back. The caret ends at the note's last byte — which is the *end* of
        // the definition's span, the one offset the query used to exclude — so
        // this is where the round trip either works or doesn't.
        let mut d = doc_with("fn_insert_return", "A claim and more.\n");
        d.caret = 7;
        d.insert_footnote();
        d.insert("the note");
        assert_eq!(d.source, "A claim[^1] and more.\n\n[^1]: the note\n");
        let back = d
            .footnote_definition_at_caret()
            .expect("still in the note we just typed");
        assert_eq!(back.label, "1");
        // …and following it lands on the reference's label, where a reader's
        // return leg lands.
        assert_eq!(back.offset, Some(9));
        assert_eq!(&d.source[9..10], "1");
    }

    #[test]
    fn insert_footnote_takes_one_undo_for_both_halves() {
        // twig writes the pair as a single edit; the point of that is here.
        let before = "A claim and more.\n";
        let mut d = doc_with("fn_insert_undo", before);
        d.caret = 7;
        d.insert_footnote();
        assert_ne!(d.source, before);
        d.undo();
        assert_eq!(d.source, before, "one undo takes back both halves");
    }

    #[test]
    fn insert_footnote_refuses_a_format_that_cannot_spell_one() {
        // HTML is authorable — it spells the inline marks — and has no footnote.
        // The refusal says so rather than writing brackets that would render as
        // brackets.
        let src = "<p>A claim.</p>\n";
        let mut d = Doc::from_source(src.to_string(), Format::Html).unwrap();
        assert!(!Capabilities::of(Format::Html).footnote);
        d.caret = 5;
        d.insert_footnote();
        assert_eq!(d.source, src, "nothing written");
        assert!(d.status.is_some_and(|s| s.starts_with("footnote:")));
    }

    #[test]
    fn insert_footnote_leaves_the_caret_on_a_real_stop_in_the_rich_view() {
        // The empty body is the one place this could go wrong: the definition
        // renders as a `[1] ` marker the caret cannot occupy, so a caret aimed a
        // byte early would draw up in the paragraph above the note it belongs to.
        let mut d = doc_in(View::Wysiwyg, "fn_insert_stop", "A claim and more.\n");
        d.place_caret(7, false);
        d.insert_footnote();
        d.build_visual(80); // the frame a frontend draws after the edit
        assert_eq!(
            d.vmap.snap_to_stop(d.caret),
            d.caret,
            "the caret sits on a stop"
        );
        let (row, _) = d.caret_pos();
        assert!(
            drawn_rows(&d)[row].contains("[1]"),
            "the caret is on the note's row, not above it: {:?}",
            drawn_rows(&d)
        );
    }

    #[test]
    fn footnote_at_caret_resolves_a_reference_to_its_note() {
        // `[^1]` spans 7..11; its label byte is at 9. The definition follows a
        // blank line, as one has to.
        let mut d = doc_with("fn_at_caret", "A claim[^1] and more.\n\n[^1]: the note\n");
        d.caret = 9;
        let f = d
            .footnote_at_caret()
            .expect("the caret stands in a reference");
        assert_eq!(f.label, "1");
        assert_eq!(f.text.as_deref(), Some("the note"));
        // The offset points at the note's first word, not at the definition's
        // `[` — the marker is decoration with no caret stop on it.
        assert_eq!(f.offset, Some(29));
        assert_eq!(&d.source[29..37], "the note");
        // …and `end` closes the range, so a frontend can ask which rendered rows
        // the note occupies rather than re-deriving them from the text.
        assert_eq!(f.end, Some(37));
        assert_eq!(&d.source[f.offset.unwrap()..f.end.unwrap()], "the note");
    }

    /// Two definitions in a row: each is its own note, and neither reaches into
    /// the other.
    ///
    /// A djot definition's span used to run past the blank line into the first
    /// byte of whatever followed, so this answered `"first note.\n\n["` — and the
    /// offsets named the *next* note's rows too, showing a reader two footnotes
    /// when they had asked about one. twig 3.1 ends the span after the block's
    /// own last line; the test outlives the workaround leaf carried for it.
    #[test]
    fn footnote_at_stops_a_note_at_the_definition_after_it() {
        let src = "Claim[^2a] and [^2b].\n\n[^2a]: first note.\n\n[^2b]: second note.\n";
        for format in [Format::Markdown, Format::Djot] {
            let mut d = Doc::from_source(src.to_string(), format).unwrap();
            d.caret = 7;
            let f = d.footnote_at_caret().expect("a reference");
            assert_eq!(f.text.as_deref(), Some("first note."), "in {format:?}");
            assert_eq!(
                &src[f.offset.unwrap()..f.end.unwrap()],
                "first note.",
                "in {format:?}"
            );
        }
    }

    /// The other side of that boundary: a blank line *inside* a definition is
    /// interior to it, and the note keeps its second paragraph.
    ///
    /// This is what the old body scan cost. It stopped at the first line not
    /// indented under the note — a blank line is not — so a two-paragraph note
    /// came back as its first paragraph, and "go to note" framed half of it.
    /// Reading the span twig gives is both simpler and right.
    #[test]
    fn footnote_at_keeps_a_notes_second_paragraph() {
        let src = "Claim[^1].\n\n[^1]: first para.\n\n    second para.\n\nAfter.\n";
        let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
        d.caret = 7;
        let f = d.footnote_at_caret().expect("a reference");
        assert_eq!(f.text.as_deref(), Some("first para.\n\n    second para."));
        // And it stops there — `After.` is the next block, not more note.
        assert_eq!(
            &src[f.offset.unwrap()..f.end.unwrap()],
            f.text.as_deref().unwrap()
        );
        assert!(!f.text.as_deref().unwrap().contains("After"));
    }

    #[test]
    fn footnote_at_bounds_a_note_whose_body_is_empty() {
        // `[^1]:` with nothing after it. The range is empty rather than
        // inverted, and still points inside the definition — which is what keeps
        // a frontend's row lookup from walking off into the block above.
        let src = "A claim[^1].\n\n[^1]:\n";
        let mut d = doc_with("fn_empty_body", src);
        d.caret = 9;
        let f = d.footnote_at_caret().expect("a reference");
        assert_eq!(f.text.as_deref(), Some(""));
        assert_eq!(f.offset, f.end, "an empty note is an empty range");
        assert!(f.offset.unwrap() >= src.find("[^1]:").unwrap());
    }

    #[test]
    fn footnote_at_caret_ignores_a_caret_that_stands_in_no_reference() {
        let mut d = doc_with(
            "fn_at_caret_none",
            "A claim[^1] and more.\n\n[^1]: the note\n",
        );
        d.caret = 2; // in the prose
        assert_eq!(d.footnote_at_caret(), None);
    }

    #[test]
    fn footnote_at_caret_is_not_a_link_query_and_vice_versa() {
        // The two are deliberately separate: a reference names a note in this
        // document, a link names somewhere to leave for, and answering one with
        // the other is what made a reference click do nothing at all.
        let mut d = doc_with("fn_vs_link", "a[^1] b [t](https://x.dev)\n\n[^1]: note\n");
        d.caret = 3; // the `1` of `[^1]`
        assert!(d.footnote_at_caret().is_some());
        assert_eq!(
            d.link_destination_at_caret(),
            None,
            "a reference is not a link"
        );

        d.caret = 10; // inside the link's label
        assert_eq!(d.footnote_at_caret(), None, "a link is not a reference");
        assert_eq!(
            d.link_destination_at_caret().as_deref(),
            Some("https://x.dev")
        );
    }

    #[test]
    fn footnote_at_caret_reports_an_undefined_reference_rather_than_nothing() {
        // A `[^99]` the document never defines is a real state — a note deleted
        // out from under its reference — and the label is what lets a frontend
        // say so. `None` here would be indistinguishable from "not on a
        // reference", which is the wrong thing to tell a reader.
        let mut d = doc_with("fn_undefined", "A claim[^99] and more.\n");
        d.caret = 9;
        let f = d
            .footnote_at_caret()
            .expect("the reference is still a reference");
        assert_eq!(f.label, "99");
        assert_eq!(f.text, None);
        assert_eq!(f.offset, None);
    }

    #[test]
    fn footnote_at_caret_reads_a_word_label_and_a_multiline_note() {
        // Labels are not always numbers, and a note's body runs past its first
        // line — the indented continuation belongs to the note, so it comes back
        // with it (source bytes, verbatim, as documented).
        let src = "see[^note] here\n\n[^note]: first line\n    second line\n";
        let mut d = doc_with("fn_word_label", src);
        d.caret = 6;
        let f = d
            .footnote_at_caret()
            .expect("the caret stands in a reference");
        assert_eq!(f.label, "note");
        assert_eq!(f.text.as_deref(), Some("first line\n    second line"));
    }

    #[test]
    fn footnote_at_answers_for_an_offset_the_caret_is_nowhere_near() {
        // The point of the offset form: a pointer hovering a reference asks what
        // note it names, and must not drag the caret along to ask.
        let mut d = doc_with("fn_at_off", "A claim[^1] and more.\n\n[^1]: the note\n");
        d.caret = 0;
        let f = d.footnote_at(9).expect("offset 9 stands in the reference");
        assert_eq!(f.label, "1");
        assert_eq!(f.text.as_deref(), Some("the note"));
        assert_eq!(d.caret, 0, "asking must not move the caret");
        assert_eq!(d.footnote_at(2), None, "offset 2 is prose");
    }

    #[test]
    fn footnote_definition_at_caret_points_back_at_the_reference() {
        // The return leg. `[^1]` spans 7..11, so its label — the only byte of it
        // the caret can rest on — is at 9.
        let mut d = doc_with("fn_def", "A claim[^1] and more.\n\n[^1]: the note\n");
        d.caret = 30; // inside the note's body
        let f = d
            .footnote_definition_at_caret()
            .expect("the caret stands in a definition");
        assert_eq!(f.label, "1");
        assert_eq!(f.offset, Some(9));
        assert_eq!(&d.source[7..11], "[^1]");
    }

    #[test]
    fn footnote_definition_at_covers_where_a_go_to_note_actually_lands() {
        // The two legs have to meet: wherever `footnote_at` sends the caret, the
        // definition query must answer for — otherwise arriving at a note leaves
        // the reader somewhere the way back isn't offered.
        let src = "A claim[^1] and more.\n\n[^1]: the note\n";
        let mut d = doc_with("fn_def_marker", src);
        let landed = d.footnote_at(9).unwrap().offset.unwrap();
        assert_eq!(
            d.footnote_definition_at(landed).and_then(|f| f.offset),
            Some(9),
            "the note a reference sends you to offers the way back"
        );
    }

    #[test]
    fn footnote_definition_at_caret_ignores_prose_and_the_reference_itself() {
        // The two queries answer for disjoint places, which is what lets one
        // gesture mean "down to the note" in one and "back up" in the other
        // without either having to remember which way the reader is going.
        let mut d = doc_with("fn_def_none", "A claim[^1] and more.\n\n[^1]: the note\n");
        d.caret = 2; // prose
        assert_eq!(d.footnote_definition_at_caret(), None);
        d.caret = 9; // the reference
        assert_eq!(d.footnote_definition_at_caret(), None);
        assert!(
            d.footnote_at_caret().is_some(),
            "which is the reference's own query"
        );
    }

    #[test]
    fn footnote_definition_at_caret_reports_an_orphan_note_rather_than_nothing() {
        // Nothing cites `[^2]`. Answering `None` would say "you are not in a
        // note", which is false and leaves a frontend unable to explain why the
        // way back is missing.
        let src = "A claim[^1].\n\n[^1]: cited\n\n[^2]: orphan\n";
        let mut d = doc_with("fn_def_orphan", src);
        d.caret = src.find("orphan").unwrap();
        let f = d
            .footnote_definition_at_caret()
            .expect("an orphan is still a definition");
        assert_eq!(f.label, "2");
        assert_eq!(f.offset, None);
    }

    #[test]
    fn footnote_definition_at_caret_returns_to_the_first_of_repeated_references() {
        // One label, cited twice. The first is where the reader most likely came
        // from, and the only answer that doesn't depend on how they got here.
        let src = "One[^a] and two[^a].\n\n[^a]: the note\n";
        let mut d = doc_with("fn_def_repeat", src);
        d.caret = src.find("the note").unwrap();
        let f = d.footnote_definition_at_caret().expect("a definition");
        assert_eq!(
            f.offset,
            Some(5),
            "the first `[^a]`'s label, not the second's"
        );
        assert_eq!(&src[3..7], "[^a]");
    }

    #[test]
    fn footnote_navigation_is_a_round_trip_through_placed_carets() {
        // Down and back up, each leg found from the document rather than from a
        // memory of the other — so it still works for a reader who scrolled to
        // the notes instead of jumping there.
        //
        // `place_caret` rather than assigning `caret`, because that is what a
        // frontend calls: it snaps to a real caret stop, and a jump that lands
        // on a byte the caret can't rest on would arrive somewhere the return
        // leg no longer answers for. `build_map` first, since snapping is a
        // no-op until the map exists — which is exactly how this went unnoticed
        // when the offsets pointed at the `[^` markers.
        let mut d = doc_with("fn_round", "A claim[^1] and more.\n\n[^1]: the note\n");
        d.build_map(None);
        d.place_caret(9, false);
        let down = d
            .footnote_at_caret()
            .expect("a reference")
            .offset
            .expect("a note");
        d.place_caret(down, false);
        let up = d
            .footnote_definition_at_caret()
            .expect("a definition")
            .offset
            .expect("a reference");
        d.place_caret(up, false);
        assert_eq!(d.caret, up, "the way back is a stop the caret can occupy");
        assert_eq!(
            d.footnote_at_caret().expect("back on the reference").label,
            "1"
        );
    }

    #[test]
    fn footnote_navigation_survives_a_surface_that_drops_the_brackets() {
        // With the brackets gone the label is the whole of what is drawn, so
        // everything that finds a reference — the round trip, a caret walking
        // through it, the reveal — has only the label's own glyphs to go by.
        let src = "A claim[^1] and more.\n\n[^1]: the note\n";
        let mut d = doc_in(View::Wysiwyg, "fn_round_raised", src);
        d.set_raised_superscripts(true);
        d.build_visual(80);
        assert!(drawn_rows(&d).iter().any(|r| r == "A claim1 and more."));
        assert!(drawn_rows(&d).iter().any(|r| r == "1 the note"));

        d.place_caret(9, false);
        assert_eq!(d.caret, 9, "the label is a stop");
        let down = d
            .footnote_at_caret()
            .expect("a reference")
            .offset
            .expect("a note");
        d.place_caret(down, false);
        let up = d
            .footnote_definition_at_caret()
            .expect("a definition")
            .offset
            .expect("a reference");
        d.place_caret(up, false);
        assert_eq!(d.caret, up);
        assert_eq!(
            d.footnote_at_caret().expect("back on the reference").label,
            "1"
        );

        // The caret walks prose, label, prose, never resting on the markup
        // either side: from before the `m`, onto the label, past it, and past
        // the space — `claim|1| and`, with the `[^` and `]` stepped over.
        d.place_caret(src.find("m[").unwrap(), false);
        let mut walk = vec![];
        for _ in 0..3 {
            d.move_right(false);
            walk.push(d.caret);
        }
        assert_eq!(walk, [9, 11, 12], "{walk:?}");

        // Revealed, the reference is its source, brackets and all, raised or not.
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "claim");
        assert!(
            drawn_rows(&d).iter().any(|r| r == "A claim[^1] and more."),
            "{:?}",
            drawn_rows(&d)
        );
    }

    #[test]
    fn insert_link_hands_the_destination_to_twig_raw() {
        // Escaping is twig's, and format-specific: Markdown ends a destination
        // at the first space and needs the `<…>` form, where djot would read
        // those angle brackets as part of the URL.
        let mut d = doc_with("link_space", "word\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("a b");
        assert_eq!(d.source, "[word](<a b>)\n");
    }

    #[test]
    fn insert_link_reports_a_destination_no_format_can_carry() {
        let mut d = doc_with("link_bad", "word\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("a\nb");
        assert_eq!(d.source, "word\n"); // untouched, not quietly rewritten
        assert!(
            d.status.is_some(),
            "InvalidArgument should reach the status line"
        );
        assert!(!d.dirty);
    }

    #[test]
    fn insert_link_works_in_wysiwyg_view() {
        let mut d = wysiwyg_doc("link_wys", "word here\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link("http://x.dev");
        assert_eq!(d.source, "[word](http://x.dev) here\n");
        assert_eq!(d.selected_text(), Some("word"));
        // The map the caret has to keep riding is rebuilt each frame; motion
        // over the fresh one must still land on a real stop (the debug_assert).
        d.build_visual(80);
        d.move_right(false);
        d.move_left(false);
    }

    #[test]
    fn insert_link_labelled_writes_the_label_and_lands_after_the_link() {
        // A host that knows the title of what it links: the label is the
        // text, and the link is finished — caret past it, nothing selected.
        for fmt in [Format::Markdown, Format::Djot] {
            let mut d = Doc::from_source("see \n".into(), fmt).unwrap();
            d.caret = 4;
            d.insert_link_labelled("2026-10-02.md", "Title");
            assert_eq!(d.source, "see [Title](2026-10-02.md)\n", "{fmt:?}");
            assert_eq!(d.selection(), None, "{fmt:?}");
            assert_eq!(d.caret, "see [Title](2026-10-02.md)".len(), "{fmt:?}");
            assert_eq!(
                d.link_destination_at(5).as_deref(),
                Some("2026-10-02.md"),
                "{fmt:?}"
            );
            assert!(d.dirty);
        }
    }

    #[test]
    fn insert_link_labelled_escapes_the_label_so_it_round_trips() {
        // A `]` must not close the link early, nor a `*` open emphasis: the
        // label goes in as literal text in the body's own grammar.
        for fmt in [Format::Markdown, Format::Djot] {
            let label = "a] *b* [c` <d>";
            let mut d = Doc::from_source("\n".into(), fmt).unwrap();
            d.caret = 0;
            d.insert_link_labelled("x.md", label);
            assert_eq!(d.selection(), None, "{fmt:?}");
            assert_eq!(d.caret, d.source.len() - 1, "{fmt:?}: {:?}", d.source);
            let link = d
                .nodes()
                .into_iter()
                .find(|n| n.kind == Kind::Link)
                .unwrap_or_else(|| panic!("{fmt:?}: no link in {:?}", d.source));
            assert_eq!(link.span, 0..d.source.len() - 1, "{fmt:?}: {:?}", d.source);
            assert_eq!(d.link_destination_at(1).as_deref(), Some("x.md"));
            // What the link's text reads as, markup and escapes aside.
            let text: String = d
                .nodes()
                .into_iter()
                .filter(|n| n.span.start >= link.span.start && n.span.end <= link.span.end)
                .inspect(|n| {
                    assert!(
                        matches!(n.kind, Kind::Para | Kind::Link | Kind::Str),
                        "{fmt:?}: the label minted {:?} in {:?}",
                        n.kind,
                        d.source
                    )
                })
                .filter(|n| n.kind == Kind::Str)
                .filter_map(|n| n.text)
                .collect();
            assert_eq!(text, label, "{fmt:?}: {:?}", d.source);
        }
    }

    #[test]
    fn insert_link_labelled_ignores_the_label_over_a_selection() {
        let mut d = doc_with("link_lab_sel", "word here\n");
        d.anchor = Some(0);
        d.caret = 4;
        d.insert_link_labelled("http://x.dev", "Ignored");
        assert_eq!(d.source, "[word](http://x.dev) here\n");
        assert_eq!(d.selected_text(), Some("word"));
    }

    #[test]
    fn insert_link_labelled_repoints_the_link_the_caret_stands_in() {
        // A host's Edit Link calls the labelled verb too: inside a link it
        // re-points and keeps the text there is, as `insert_link` does.
        let mut d = doc_with("link_lab_repoint", "[word](http://x.dev)\n");
        d.caret = 3;
        d.insert_link_labelled("http://y.dev", "Ignored");
        assert_eq!(d.source, "[word](http://y.dev)\n");
        assert_eq!(d.selected_text(), Some("word"));

        let mut d = doc_with("link_lab_repoint_auto", "see <https://x.dev> ok\n");
        d.caret = 10;
        d.insert_link_labelled("https://y.dev", "Ignored");
        assert_eq!(d.source, "see <https://y.dev> ok\n");
    }

    #[test]
    fn insert_link_labelled_just_past_a_link_writes_a_new_one() {
        // The caret at a link's end is not in it: nothing to re-point.
        let mut d = doc_with("link_lab_after", "[a](x.md)\n");
        d.caret = 9;
        d.insert_link_labelled("y.md", "B");
        assert_eq!(d.source, "[a](x.md)[B](y.md)\n");
        assert_eq!(d.caret, 18);
    }

    #[test]
    fn insert_link_labelled_is_one_undo_step() {
        let mut d = doc_with("link_lab_undo", "see \n");
        d.caret = 4;
        let steps = d.undo_steps;
        d.insert_link_labelled("x.md", "Title");
        assert_eq!(d.undo_steps, steps + 1);
        assert!(d.undo());
        assert_eq!(d.source, "see \n");
        assert_eq!(d.caret, 4);
        assert!(d.redo());
        assert_eq!(d.source, "see [Title](x.md)\n");
    }

    #[test]
    fn insert_link_labelled_refuses_a_bad_destination_and_leaves_no_label() {
        let mut d = doc_with("link_lab_bad", "see \n");
        d.caret = 4;
        d.insert_link_labelled("a\nb", "Title");
        assert_eq!(d.source, "see \n");
        assert!(d.status.is_some());
        assert!(!d.dirty);
    }

    #[test]
    fn insert_link_labelled_refuses_a_read_only_document() {
        let mut d = doc_with("link_lab_ro", "see \n");
        d.set_read_only(true);
        d.caret = 4;
        d.insert_link_labelled("x.md", "Title");
        assert_eq!(d.source, "see \n");
        assert!(!d.dirty);
    }

    #[test]
    fn click_maps_a_row_col_to_a_byte_offset() {
        let mut d = doc_with("click", "ab\ncd\n");
        d.click(1, 1, false); // row 1 ("cd"), col 1 -> the 'd'
        assert_eq!(d.caret, 4);
    }

    // A pixel-hit-test placement (the GUI's `place_caret`) must land on a caret
    // stop just as the `(row, col)` click path does, so the caret can never come
    // to rest in the blank gap between two paragraphs — where it would draw in one
    // place and type in another.
    #[test]
    fn place_caret_snaps_out_of_the_blank_gap_between_paragraphs() {
        // "A\n\nB": offset 2 is the gap the paragraph break is drawn with, not a
        // caret stop (stops are 0,1,3,4).
        let mut d = wysiwyg_doc("place_gap", "A\n\nB");
        assert!(!d.vmap.is_stop(2), "offset 2 should be an unreachable gap");
        d.place_caret(2, false);
        assert!(d.vmap.is_stop(d.caret), "caret {} is not a stop", d.caret);
        assert_eq!(d.caret, 1, "should snap to the end of the paragraph above");
    }

    #[test]
    fn place_caret_dragging_through_the_gap_keeps_selection_on_stops() {
        let mut d = wysiwyg_doc("place_gap_drag", "A\n\nB");
        d.place_caret(0, false); // anchor at the start of "A"
        d.place_caret(2, true); // drag into the gap
        assert!(d.vmap.is_stop(d.caret), "caret {} is not a stop", d.caret);
        let (s, e) = d.selection().expect("a selection");
        assert!(
            d.vmap.is_stop(s) && d.vmap.is_stop(e),
            "selection {s}..{e} off a stop"
        );
    }

    #[test]
    fn place_caret_on_a_real_stop_is_left_untouched() {
        let mut d = wysiwyg_doc("place_stop", "A\n\nB");
        d.place_caret(3, false); // the start of "B" — a genuine stop
        assert_eq!(d.caret, 3);
    }

    // An *empty paragraph* (two blank lines, an intentional blank line the user
    // opened) is a real caret stop, unlike the gap — a click into it must stay.
    #[test]
    fn place_caret_rests_in_an_empty_paragraph() {
        let mut d = wysiwyg_doc("place_empty_para", "A\n\n\n\nB");
        let empty = 3; // the navigable empty row's offset (stops: 0,1,3,5,6)
        assert!(d.vmap.is_stop(empty));
        d.place_caret(empty, false);
        assert_eq!(d.caret, empty);
    }

    // The content end of a hidden mark is a home too (`VisualMap::mark_ends`):
    // a drag over the word `bold` ends there, and a caret placed there stays.
    #[test]
    fn place_caret_rests_at_the_end_of_a_hidden_marks_content() {
        let src = "| A | B |\n| --- | --- |\n| **bold** | other |\n";
        let mut d = wysiwyg_doc("place_mark_end", src);
        let start = src.find("bold").unwrap();
        d.place_caret(start, false);
        d.place_caret(start + 4, true);
        assert_eq!(d.selection(), Some((start, start + 4)), "the whole word");
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, src.replace("**bold**", "bold"));
    }

    #[test]
    fn right_steps_onto_the_end_of_a_mark_and_then_past_its_delimiter() {
        let mut d = wysiwyg_doc("right_mark_end", "a **bold** b");
        d.caret = 7; // before the `d`
        d.move_right(false);
        assert_eq!(d.caret, 8, "onto the end of the bold");
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.move_right(false);
        assert_eq!(d.caret, 10, "past the closing `**`");
        assert!(!d.active_inline_marks().contains(InlineKind::Strong));
        d.move_left(false);
        assert_eq!(d.caret, 8);
        d.move_left(false);
        assert_eq!(d.caret, 7);
        // Typing at the inner home extends the bold.
        d.caret = 8;
        d.insert("!");
        assert_eq!(d.source, "a **bold!** b");
    }

    #[test]
    fn a_marks_end_home_follows_an_edit_through_the_incremental_map() {
        // The splice path shifts the home with the block it is in, and the
        // re-rendered block finds its own again.
        let mut d = wysiwyg_doc("mark_end_splice", "x\n\na **bold** b\n\ny\n");
        d.build_visual_unwrapped();
        d.edit(0, 0, "zz");
        d.build_visual_unwrapped();
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after a shift");
        assert!(d.vmap.is_stop(d.source.find("bold").unwrap() + 4));
        let at = d.source.find("bold").unwrap();
        d.edit(at, at, "very ");
        d.build_visual_unwrapped();
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after a re-render");
        assert!(d.vmap.is_stop(d.source.find("bold").unwrap() + 4));
    }

    fn wysiwyg_doc(name: &str, body: &str) -> Doc {
        doc_in(View::Wysiwyg, name, body)
    }

    /// How many list items the source actually parses into — the check that a
    /// marker Leaf wrote is a marker the format agrees is one.
    fn list_items(doc: &mut Doc) -> usize {
        doc.editor
            .nodes()
            .unwrap()
            .iter()
            .filter(|n| n.kind == Kind::ListItem || n.kind == Kind::TaskListItem)
            .count()
    }

    /// A from-scratch, cache-free WYSIWYG map for `source` — the ground truth the
    /// incremental (`build_spliced` / `build_cached`) path must always match.
    fn reference_map(source: &str) -> crate::wysiwyg::VisualMap {
        reference_map_revealing(source, None)
    }

    /// [`reference_map`] with a reveal line — the ground truth for the
    /// `MarkupMode::Full` builds, where the map is a function of the caret's
    /// line as well as the text.
    fn reference_map_revealing(source: &str, reveal: Option<Reveal>) -> crate::wysiwyg::VisualMap {
        // The same parse `Doc` uses. With twig's plain defaults instead, the two
        // sides disagree on what the *document* is before the renderer is even
        // reached — a bare `:word` is a text directive to one and prose to the
        // other — and the mismatch reads as a splice bug that isn't one.
        let mut ed =
            twig::Editor::new_ext(source.as_bytes(), Format::Markdown, parse_extensions()).unwrap();
        let nodes = ed.nodes().unwrap();
        crate::wysiwyg::build(
            &nodes,
            source,
            None,
            false,
            &wysiwyg::Surface::default(),
            reveal,
        )
    }

    fn maps_differ(a: &crate::wysiwyg::VisualMap, b: &crate::wysiwyg::VisualMap) -> bool {
        if a.rows.len() != b.rows.len() {
            return true;
        }
        for (ra, rb) in a.rows.iter().zip(&b.rows) {
            if ra.end_src != rb.end_src || ra.glyphs.len() != rb.glyphs.len() {
                return true;
            }
            for (ga, gb) in ra.glyphs.iter().zip(&rb.glyphs) {
                if ga.ch != gb.ch || ga.src != gb.src {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn incremental_build_matches_a_fresh_build_across_edits() {
        // Every `Doc` edit rebuilds through `build_spliced` (the single-block
        // fast path, gated on twig's `dirty_range`) or falls back to
        // `build_cached`. After each edit the map must be byte-identical to a
        // from-scratch build — this is the correctness net under the splice.
        let docs = [
            "# Title\n\nThe quick brown fox jumps.\n\nAnother paragraph here.\n\n- a\n- b\n",
            "para one\n\n> quote **bold** text\n> continued line\n\ntail paragraph\n",
            "alpha\n\nbeta\n\ngamma\n\ndelta\n\nepsilon\n\nzeta\n",
            // A footnote definition is a root beside `doc`, merged back into the
            // top-level list by `wysiwyg::top_blocks`. The random edits below
            // make and unmake definitions as they go (a deleted `:` turns one
            // back into a paragraph, and vice versa), which is exactly the
            // structural churn the splice path has to notice and bail out of.
            "text[^1] here\n\n[^1]: the note\n\nmore text[^b]\n\n[^b]: second\n",
            // A comment is a top-level block that draws no rows — a layout entry
            // at zero rows either side of blocks that do. The edits below type
            // into the blocks around it (a splice past a hidden block), and
            // break the comment open into prose and back (a structural change).
            "intro\n\n<!-- exec -->\n```\ncode\n```\n\nafter the comment\n\n<!-- trail -->\n",
            // Link reference definitions: a hidden block that an edit can turn
            // into a paragraph (a deleted `:`) and back, and whose own bytes an
            // edit can land in.
            "see [a] and [b]\n\n[a]: /a\n\nmid text\n\n[b]: /b\n",
            // Blank lines above the first block draw as rows, counted as its
            // separator; above it past frontmatter and past a comment too.
            "\n\nfirst pushed down\n\nsecond\n",
            "---\ntitle: x\n---\n\n\nafter frontmatter\n\nmore\n",
            "<!-- lead -->\n\n\nafter a comment\n\nmore\n",
            // No `<div>` here yet: an edit that takes the blank line under
            // `</div>` leaves a map no fresh build draws. See
            // docs/tasks/text-glued-under-a-closing-div.md.
        ];
        // A deterministic mix: mostly single characters (which stay inside one
        // block → splice), plus edits that reshape structure (a paragraph break,
        // a heading marker, a code fence → fallback), so both paths are exercised.
        let inserts = ["x", "y", "\n\n", "#", "`", " ", "z"];
        for src in docs {
            let mut d = wysiwyg_doc("diff", src);
            d.build_visual_unwrapped();
            wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "initial");

            for step in 0..60usize {
                let len = d.source.len();
                let raw = (step * 13 + 5) % (len + 1);
                let pos = (raw..=len).find(|&i| d.source.is_char_boundary(i)).unwrap();
                let pre = d.source.clone();
                let action;
                if step % 3 == 0 && pos < len {
                    let end = (pos + 1..=len)
                        .find(|&i| d.source.is_char_boundary(i))
                        .unwrap();
                    action = format!("delete [{pos},{end})");
                    d.edit(pos, end, "");
                } else {
                    let ins = inserts[step % inserts.len()];
                    action = format!("insert {ins:?} @ {pos}");
                    d.edit(pos, pos, ins);
                }
                d.build_visual_unwrapped();
                if maps_differ(&d.vmap, &reference_map(&d.source)) {
                    panic!(
                        "FIRST MISMATCH at step {step}: {action}\n  pre  = {pre:?}\n  post = {:?}",
                        d.source
                    );
                }
            }
        }
    }

    /// A frontend is handed [`Doc::vmap`] and may present it differently:
    /// leaf-ratatui splices blank filler rows under an oversized heading so the
    /// raster it paints there has somewhere to stand, and leaves them in the map
    /// because the caret and the mouse both read it between frames. The splice
    /// path addresses that map by *row index*, against the block layout the last
    /// build recorded — so handed a map with rows in it that no block owns, it
    /// laid the re-rendered block over one of the fillers and carried the rows
    /// the block really occupied into the suffix. One stranded copy of the
    /// edited line, and everything below it a row further down, per keystroke.
    ///
    /// A map that isn't the one the layout describes is a map this path can't
    /// patch, whoever changed it and for whatever reason. It rebuilds instead.
    #[test]
    fn an_edit_over_a_map_a_frontend_reshaped_rebuilds_it_whole() {
        let mut d = wysiwyg_doc("reshaped", "# Title\n\nThe quick brown fox jumps.\n");
        d.build_visual_unwrapped();

        // Stand in for the heading filler rows: two blank rows past the heading
        // that no block accounts for. Cloning a real row keeps every field
        // plausible — it is the row *count* the splice can't survive.
        let filler = d.vmap.rows[0].clone();
        d.vmap.rows.insert(1, filler.clone());
        d.vmap.rows.insert(1, filler);

        // An edit inside the last block: the single-block case the splice path
        // is for, and the one the frontend hits on every keystroke.
        let at = d.source.len() - 1;
        d.edit(at, at, "!");
        d.build_visual_unwrapped();

        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after the edit");
    }

    /// A glyph's [`FaceId`] has to mean the same thing however its row was
    /// built. A row comes three ways — a fresh walk, a [`BlockCache`] hit
    /// cloned at a shifted offset, and a previous map's rows a splice kept
    /// untouched — and only the first of those walks a `data-font` at all. An
    /// index into a per-build table would have had the same glyph naming two
    /// families the moment a second one appeared; the id is the name's own
    /// hash, so nothing is remapped and the table is merged rather than rebuilt.
    ///
    /// Two families, because one cannot tell a wrong id from a right one.
    ///
    /// [`FaceId`]: crate::style::FaceId
    /// [`BlockCache`]: crate::wysiwyg::BlockCache
    #[test]
    fn a_spliced_rebuild_still_says_which_family_each_glyph_is_set_in() {
        use crate::style::{FaceId, FaceRef};
        let garamond = FaceId::of("Garamond");
        let futura = FaceId::of("Futura");
        let mut d = wysiwyg_doc(
            "two_faces",
            "x <span data-font=\"Garamond\">alpha</span>\n\ny <span data-font=\"Futura\">beta</span>\n",
        );
        d.build_visual_unwrapped();

        // What the map has to keep saying, whichever path built it.
        let check = |d: &Doc, ctx: &str| {
            let face_of = |ch: char| {
                d.vmap
                    .rows
                    .iter()
                    .flat_map(|r| r.glyphs.iter())
                    .find(|g| g.ch == ch)
                    .map(|g| g.style.font)
            };
            assert_eq!(face_of('a'), Some(Some(FaceRef::Named(garamond))), "{ctx}");
            assert_eq!(face_of('b'), Some(Some(FaceRef::Named(futura))), "{ctx}");
            assert_eq!(d.vmap.face_name(garamond), Some("Garamond"), "{ctx}");
            assert_eq!(d.vmap.face_name(futura), Some("Futura"), "{ctx}");
            assert_eq!(d.vmap.face_name(FaceId::of("Bodoni")), None, "{ctx}");
        };
        check(&d, "fresh");

        // An edit inside the second block: the single-block case the splice
        // path is for. The first block's rows are carried over untouched, so
        // its glyphs' ids are the previous build's and the table has to be too.
        let at = d.source.find("beta").unwrap();
        d.edit(at, at, "z");
        d.build_visual_unwrapped();
        check(&d, "after an edit in the second block");
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "spliced");

        // And the other way round, so the block that was kept is the one that
        // is now re-rendered.
        let at = d.source.find("alpha").unwrap();
        d.edit(at, at, "z");
        d.build_visual_unwrapped();
        check(&d, "after an edit in the first block");
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "spliced again");

        // A structural edit is one the splice bails out of, so the map is
        // reassembled by `build_cached` — where an untouched block is a *cache
        // hit* and its rows are cloned without a `data-font` being walked
        // again. The names the entry stored are what keeps the table honest
        // there.
        let at = d.source.find("\n\ny ").unwrap();
        d.edit(at, at, "\n\nmiddle");
        d.build_visual_unwrapped();
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "cached");
        // Twice, because that first `build_cached` is what stores the entries:
        // this one is the build where the Garamond block is a *hit*, its rows
        // cloned with their ids and no attribute walked to explain them.
        let at = d.source.len() - 1;
        d.edit(at, at, "\n\ntail");
        d.build_visual_unwrapped();
        check(&d, "after a structural edit, through the block cache");
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "cached again");

        // A family the edit took the last glyph of leaves the glyphs with no
        // face and the table with a name nothing asks for — harmless, and the
        // price of not walking the rows the splice exists to avoid walking.
        let span = d.source.find("<span data-font=\"Futura\">").unwrap();
        let end = d.source.rfind("</span>").unwrap() + "</span>".len();
        d.edit(span, end, "beta");
        d.build_visual_unwrapped();
        assert!(!d.source.contains("Futura"), "{:?}", d.source);
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "the face removed");
    }

    #[test]
    fn incremental_build_matches_a_fresh_build_under_full_reveal() {
        // The same correctness net as `incremental_build_matches_a_fresh_build_
        // across_edits`, under `MarkupMode::Full` — where the map depends on
        // the caret's *line* as well as the text, so the two caches have a new
        // way to be wrong. Both are exercised: the block cache can hand back
        // rows built for a line that is no longer the revealed one, and the
        // splice path can reuse a suffix that still has yesterday's line raw.
        //
        // Caret motion is interleaved with the edits deliberately, because a
        // caret that only ever moved with the edit would never cross a line
        // without also dirtying it — the case where a stale reveal survives.
        let docs = [
            "# Title\n\n*one* and **two**\n\n[lk](http://x) and `code`\n\n- a *b*\n",
            "para *em* one\n\n> quote **bold** text\n\ntail ~~del~~ paragraph\n",
        ];
        let inserts = ["x", "*", "\n\n", "#", "`", " ", "_"];
        for src in docs {
            let mut d = wysiwyg_doc("reveal_diff", src);
            d.set_markup_mode(MarkupMode::Full);

            for step in 0..60usize {
                let len = d.source.len();
                let raw = (step * 13 + 5) % (len + 1);
                let pos = (raw..=len).find(|&i| d.source.is_char_boundary(i)).unwrap();
                let pre = d.source.clone();
                let action;
                if step % 3 == 0 && pos < len {
                    let end = (pos + 1..=len)
                        .find(|&i| d.source.is_char_boundary(i))
                        .unwrap();
                    action = format!("delete [{pos},{end})");
                    d.edit(pos, end, "");
                } else {
                    let ins = inserts[step % inserts.len()];
                    action = format!("insert {ins:?} @ {pos}");
                    d.edit(pos, pos, ins);
                }
                // Walk the caret somewhere else in the document, independently
                // of where the edit landed.
                let want = (step * 29 + 11) % (d.source.len() + 1);
                d.caret = (want..=d.source.len())
                    .find(|&i| d.source.is_char_boundary(i))
                    .unwrap();
                d.build_visual_unwrapped();

                let want = reference_map_revealing(&d.source, d.reveal_line());
                if maps_differ(&d.vmap, &want) {
                    panic!(
                        "FIRST MISMATCH at step {step}: {action}, caret {}\n  pre  = {pre:?}\n  post = {:?}",
                        d.caret, d.source
                    );
                }
            }
        }
    }

    #[test]
    fn caret_motion_across_lines_rebuilds_only_under_full() {
        // The cache-key change has to earn its keep in both directions: `Full`
        // must rebuild when the caret changes line (or the reveal would never
        // move), and the hidden modes must *not* (or every arrow key would pay
        // for a feature they don't use). The existing `cache_motion` test pins
        // the second for the default mode; this pins the pair against a mode
        // change alone.
        let body = "*one* here\n\n*two* there\n";

        let mut full = doc_in(View::Wysiwyg, "motion_full", body);
        full.set_markup_mode(MarkupMode::Full);
        caret_at(&mut full, "one");
        let before = full.revision();
        caret_at(&mut full, "two");
        assert_eq!(full.revision(), before, "motion is not an edit");
        assert!(
            drawn_rows(&full).iter().any(|r| r == "*two* there"),
            "the map followed the caret: {:?}",
            drawn_rows(&full)
        );

        let mut hidden = doc_in(View::Wysiwyg, "motion_hidden", body);
        caret_at(&mut hidden, "one");
        let key = hidden.vmap_key.clone();
        caret_at(&mut hidden, "two");
        assert_eq!(
            hidden.vmap_key, key,
            "a hidden mode rebuilds nothing on motion"
        );
    }

    #[test]
    fn wysiwyg_down_crosses_a_paragraph_boundary() {
        // Regression: the blank separator row used to share the previous
        // paragraph's end offset, so Down got pinned at the boundary (while Up
        // still crossed). Both directions must step through it symmetrically.
        //
        // It's now stepped *over* rather than onto: the blank line between two
        // paragraphs is the boundary being drawn, not a line of the document, so
        // one press of Down crosses it. The goal column survives the crossing —
        // col 3 at the end of "abc" is col 3 at the end of "def".
        let mut d = wysiwyg_doc("wys_down", "abc\n\ndef\n");
        d.caret = 3; // end of "abc" (row 0)
        d.move_down(false);
        assert_eq!(d.caret_pos().0, 2, "Down should reach the second paragraph");
        assert_eq!(d.caret, 8); // end of "def", col 3 kept
        d.move_up(false);
        assert_eq!(d.caret_pos().0, 0, "Up should come back symmetrically");
        assert_eq!(d.caret, 3);
    }

    #[test]
    fn wysiwyg_up_and_down_are_inverse_across_paragraphs() {
        // The second Up and the second Down here run off the ends of the
        // document, which is no longer a place a press is swallowed: they carry
        // the caret to the start and the end of the text. The claim in the
        // middle — that a Down retraces the Up that crossed the paragraph gap —
        // is the one this test is for, and it is asserted where it is made.
        let mut d = wysiwyg_doc("wys_updown", "abc\n\ndef\n");
        d.caret = 5; // start of "def"
        let start = d.caret_pos();
        d.move_up(false);
        assert_eq!(d.caret_pos().0, 0, "Up reaches the first paragraph");
        d.move_up(false);
        assert_eq!(d.caret, 0, "a second Up runs on to the document's start");
        d.move_down(false);
        assert_eq!(d.caret_pos(), start, "Down retraces Up exactly");
        d.move_down(false);
        assert_eq!(d.caret, 8, "a second Down runs on to the document's end");
    }

    #[test]
    fn wysiwyg_new_paragraph_shows_before_typing() {
        // Regression: two Enters at the end of a paragraph produced trailing
        // newlines with no AST node, so the caret appeared stuck on the old line
        // until a character was typed. It must ride down onto the new line now.
        let mut d = doc_with("wys_newpara", "abc\n");
        d.view = View::Wysiwyg;
        d.caret = 3;
        d.insert("\n");
        d.insert("\n"); // source is now "abc\n\n\n", caret at 5
        assert_eq!(d.source, "abc\n\n\n");
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(
            row >= 2,
            "caret should have moved down to the new line, got row {row}"
        );
        assert!(
            d.vmap.num_rows() >= 3,
            "the blank lines should render as rows"
        );
    }

    #[test]
    fn wysiwyg_enter_between_paragraphs_lands_on_an_empty_line() {
        // The reported bug: Enter at the end of a paragraph that has another
        // paragraph below put the caret at the *start of the next paragraph* —
        // the empty paragraph it opened had no row, so the caret snapped onto
        // "World". It must now sit on its own empty line, with a blank spacer
        // above it (the paragraph gap).
        let mut d = wysiwyg_doc("wys_gap_mid", "Hello\n\nWorld\n");
        d.caret = 5; // end of "Hello"
        d.newline();
        d.build_visual(80);
        let (row, col) = d.caret_pos();
        assert_eq!(col, 0, "caret should start an empty line, not sit in text");
        assert_eq!(
            d.vmap.row_width(row),
            0,
            "caret's row must be empty, not 'World'"
        );
        assert!(
            row >= 2,
            "a blank spacer row should sit above the caret, got row {row}"
        );
        // The row above the caret is a real (empty) gap, and "Hello" stays put.
        assert_eq!(
            d.vmap.row_width(row - 1),
            0,
            "the row above the caret is a gap"
        );
        let row0: String = d.vmap.rows[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(row0, "Hello", "the paragraph above the caret must not move");
    }

    #[test]
    fn wysiwyg_enter_at_eof_shows_a_gap_before_typing() {
        // At the document end a single Enter must also show the paragraph gap —
        // a blank spacer row above the caret — so the layout already matches how
        // it will look once the new paragraph has text.
        let mut d = wysiwyg_doc("wys_gap_eof", "Hello");
        d.caret = 5; // end of "Hello", no trailing newline
        d.newline(); // source becomes "Hello\n\n"
        d.build_visual(80);
        let (row, col) = d.caret_pos();
        assert_eq!(col, 0);
        assert!(
            row >= 2,
            "caret should sit below a blank spacer, got row {row}"
        );
        assert_eq!(
            d.vmap.row_width(row - 1),
            0,
            "the row above the caret is a gap"
        );
    }

    #[test]
    fn wysiwyg_typing_after_enter_does_not_shift_the_caret_row() {
        // The spacer is view-only: typing the new paragraph must not reflow the
        // caret onto a different row — the transient view already matched the
        // settled one.
        let mut d = wysiwyg_doc("wys_no_reflow", "Hello\n\nWorld\n");
        d.caret = 5;
        d.newline();
        d.build_visual(80);
        let before = d.caret_pos();
        d.insert("New");
        d.build_visual(80);
        let after = d.caret_pos();
        assert_eq!(
            after.0, before.0,
            "typing must not move the caret to another row ({before:?} -> {after:?})"
        );
    }

    #[test]
    fn wysiwyg_return_on_the_last_code_line_keeps_the_caret_in_the_block() {
        // Return at the end of the block's last line writes an empty line the
        // map used to drop, so the caret landed on `after` and the next
        // keystroke went into the paragraph below instead of into the code.
        let mut d = wysiwyg_doc("code_return", "prose\n\n```\nalpha\nbeta\n```\n\nafter\n");
        d.caret = d.source.find("beta").unwrap() + "beta".len();
        d.build_visual(80);
        let before = d.caret_pos().0;

        d.newline();
        d.build_visual(80);
        assert_eq!(d.source, "prose\n\n```\nalpha\nbeta\n\n```\n\nafter\n");

        let (row, col) = d.caret_pos();
        assert_eq!(row, before + 1, "the caret moves down one row");
        assert_eq!(col, 0, "onto the head of the empty line");
        let span = d.vmap.code_blocks[0].rows_span.clone();
        assert!(
            span.contains(&row),
            "caret row {row} is outside the block's rows {span:?}"
        );

        // The whole point: what is typed next is code.
        d.insert("gamma");
        assert_eq!(d.source, "prose\n\n```\nalpha\nbeta\ngamma\n```\n\nafter\n");
    }

    #[test]
    fn wysiwyg_hides_frontmatter_from_the_caret_and_copy() {
        let fm = "---\ntitle: hi\n---\n";
        let body = format!("{fm}# leaf\n\nbody\n");
        let mut d = wysiwyg_doc("wys_fm", &body);
        // Opening lifts the caret out of the now-hidden frontmatter.
        assert_eq!(
            d.caret,
            fm.len(),
            "caret should start at the first real block"
        );
        // Left at the content start can't step back into frontmatter.
        d.move_left(false);
        assert_eq!(d.caret, fm.len(), "left must not enter frontmatter");
        // Doc-start lands on the content floor, not offset 0.
        d.move_doc_start(false);
        assert_eq!(d.caret, fm.len());
        // Select-all + copy never include the frontmatter bytes.
        d.select_all();
        let sel = d.selected_text().unwrap().to_string();
        assert!(!sel.contains("title"), "copy leaked frontmatter: {sel:?}");
        assert!(
            sel.starts_with("# leaf"),
            "selection should begin at content: {sel:?}"
        );
    }

    #[test]
    fn typing_in_a_frontmatter_only_document_lands_after_the_frontmatter() {
        // A fresh note is frontmatter and nothing else. With no rendered block
        // to floor the caret it opened at offset 0 — before the opening `---` —
        // so the first keystroke wrote itself in front of the metadata and the
        // file came out as `This---\ntitle: …`.
        let fm = "---\ntitle: 2026-08-29\nid: f8s32cd\n---\n";
        let mut d = wysiwyg_doc("wys_fm_only", fm);
        assert_eq!(d.caret, fm.len(), "caret must open past the frontmatter");
        // Nothing is rendered, so the caret draws at the origin of an empty view
        // — the same place an empty document puts it.
        assert_eq!(d.caret_pos(), (0, 0));
        d.insert("This");
        assert_eq!(d.source, format!("{fm}This"));
    }

    /// `select_range` is the verb for a range a host already knows the bytes of,
    /// so it must not snap — and must still hold every invariant `place_caret`
    /// holds, the frontmatter floor above all.
    #[test]
    fn select_range_takes_the_range_as_given_but_still_floors_it() {
        let fm = "---\ntitle: foo\n---\n\n";
        let body = format!("{fm}body foo here\n");
        let mut d = wysiwyg_doc("wys_select_range", &body);

        // The `foo` in the body: taken exactly, not snapped to a caret stop.
        let at = body.rfind("foo").unwrap();
        d.select_range(at, at + 3);
        assert_eq!(d.selection(), Some((at, at + 3)));
        assert_eq!(d.selected_text(), Some("foo"));

        // The `foo` in the hidden frontmatter: below the floor, so both ends
        // come up to it rather than parking the caret in the metadata, where a
        // later keystroke would rewrite `title:`.
        let hidden = body.find("foo").unwrap();
        assert!(hidden < d.vmap.content_start);
        d.select_range(hidden, hidden + 3);
        assert!(
            d.caret >= d.vmap.content_start && d.anchor.unwrap() >= d.vmap.content_start,
            "a range under the floor must not leave the caret in the frontmatter"
        );

        // Past the end, and mid-character, are both brought back to something
        // sliceable rather than panicking the next reader of the range.
        let multi = wysiwyg_doc("wys_select_range_utf8", "héllo\n");
        let mut d = multi;
        d.select_range(2, 9_999);
        assert_eq!(d.caret, d.source.len());
        assert!(d.source.is_char_boundary(d.anchor.unwrap()));
        assert!(d.source.is_char_boundary(d.caret));
    }

    /// The bug `select_range` exists for: a match butting up against a hidden
    /// delimiter. `place_caret` snaps to the nearest *visible* stop, which is
    /// the one before the `**`.
    #[test]
    fn select_range_does_not_snap_off_a_hidden_delimiter() {
        let mut d = wysiwyg_doc("wys_select_range_bold", "a **needle** in it\n");
        let at = d.source.find("needle").unwrap();
        d.select_range(at, at + 6);
        assert_eq!(d.selected_text(), Some("needle"), "not \"needl\"");
    }

    #[test]
    fn wysiwyg_backspace_at_content_start_leaves_frontmatter_intact() {
        // Backspace deletes `prev_boundary..caret` directly; at the first real
        // block that boundary is inside the hidden frontmatter, so it must be a
        // no-op rather than eating the closing `---`.
        let fm = "---\ntitle: hi\n---\n";
        let body = format!("{fm}leaf\n");
        let mut d = wysiwyg_doc("wys_fm_bs", &body);
        assert_eq!(d.caret, fm.len());
        d.backspace();
        assert_eq!(d.source, body, "backspace must not touch frontmatter");
        d.delete_word_back();
        assert_eq!(
            d.source, body,
            "word-delete must not touch frontmatter either"
        );
    }

    #[test]
    fn wysiwyg_edits_inside_a_vis_directive_block_without_disturbing_its_fences() {
        // diaryx's `:::vis{.audience}` visibility block — any `:::name{.class}`
        // fenced div, really, since core parses these on for every document
        // now (`parse_extensions`). The container is a `directive` node, an
        // `is_block_container` kind like `block_quote`, so the caret works
        // inside its child paragraph exactly as it would inside a quote: typing
        // edits the paragraph, and the `:::vis{...}` / `:::` fences round-trip
        // untouched.
        let body = ":::vis{.public .family}\nhello\n:::\nafter\n";
        let mut d = wysiwyg_doc("wys_vis", body);
        d.caret = body.find("hello").unwrap() + "hello".len();
        d.insert("!");
        assert_eq!(
            d.source, ":::vis{.public .family}\nhello!\n:::\nafter\n",
            "typing inside the block edits its content in place"
        );
        assert!(
            d.source.contains(":::vis{.public .family}"),
            "opening fence survives"
        );
        assert!(d.source.contains(":::\nafter"), "closing fence survives");
    }

    #[test]
    fn source_view_still_reaches_frontmatter() {
        // The metadata is only *hidden*, never lost: the source view edits and
        // selects it in full, and it's always preserved on save.
        let fm = "---\ntitle: hi\n---\n";
        let body = format!("{fm}# leaf\n");
        let mut d = doc_with("src_fm", &body);
        d.select_all();
        let sel = d.selected_text().unwrap();
        assert!(
            sel.contains("title"),
            "source view should select everything"
        );
        d.move_doc_start(false);
        assert_eq!(d.caret, 0, "source view can reach offset 0");
    }

    const TABLE: &str = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n| Fig | 12 |\n";

    #[test]
    fn wysiwyg_right_crosses_a_cell_border_without_stalling() {
        // The border and padding between two cells all share one source offset,
        // so a column-stepping caret would sit on `│` and then stall there
        // forever. Right must step: end of "Name" -> start of "Qty".
        let mut d = wysiwyg_doc("tbl_right", TABLE);
        d.caret = TABLE.find("Name").unwrap() + 4; // just after "Name"
        d.move_right(false);
        assert_eq!(
            d.caret,
            TABLE.find("Qty").unwrap(),
            "should land in the next cell"
        );
        let (r, c) = d.caret_pos();
        assert_eq!(d.vmap.rows[r].glyphs[c].ch, 'Q');
    }

    #[test]
    fn wysiwyg_left_crosses_back_to_the_previous_cell() {
        let mut d = wysiwyg_doc("tbl_left", TABLE);
        d.caret = TABLE.find("Qty").unwrap();
        d.move_left(false);
        assert_eq!(
            d.caret,
            TABLE.find("Name").unwrap() + 4,
            "end of the previous cell"
        );
    }

    #[test]
    fn wysiwyg_down_steps_over_a_table_rule() {
        // Between the header and the first body row sits a `├───┼───┤` rule.
        // It's drawn but holds no caret, so one Down must reach "Pear".
        let mut d = wysiwyg_doc("tbl_down", TABLE);
        d.caret = TABLE.find("Name").unwrap();
        d.move_down(false);
        assert_eq!(
            d.caret,
            TABLE.find("Pear").unwrap(),
            "one Down reaches the body row"
        );
        d.move_down(false);
        assert_eq!(d.caret, TABLE.find("Fig").unwrap());
    }

    #[test]
    fn wysiwyg_tab_walks_the_cells_and_shift_tab_walks_back() {
        let mut d = wysiwyg_doc("tbl_tab", TABLE);
        d.caret = TABLE.find("Name").unwrap();
        // A hop lands with the destination cell's whole content selected, the
        // caret at its end — so typing replaces the cell like a form field.
        assert!(d.cell_hop(true));
        assert_eq!(
            d.selected_text(),
            Some("Qty"),
            "the target cell comes up selected"
        );
        assert_eq!(d.caret, TABLE.find("Qty").unwrap() + "Qty".len());
        assert!(d.cell_hop(true), "Tab wraps onto the next row's first cell");
        assert_eq!(d.selected_text(), Some("Pear"));
        assert!(d.cell_hop(false));
        assert_eq!(d.selected_text(), Some("Qty"));
    }

    #[test]
    fn tab_outside_a_table_is_not_a_cell_hop() {
        // `cell_hop` reports false so the frontend can indent as usual.
        let mut d = wysiwyg_doc("tbl_none", "just a paragraph\n");
        d.caret = 4;
        assert!(!d.cell_hop(true));
        assert_eq!(d.caret, 4, "a refused hop leaves the caret alone");
    }

    #[test]
    fn tab_at_the_last_cell_declines_rather_than_leaving_the_table() {
        let mut d = wysiwyg_doc("tbl_edge", TABLE);
        d.caret = TABLE.rfind("12").unwrap(); // the final cell
        assert!(!d.cell_hop(true), "no cell after the last one");
        d.caret = TABLE.find("Name").unwrap();
        assert!(!d.cell_hop(false), "no cell before the first one");
    }

    #[test]
    fn wysiwyg_vertical_cell_motion_holds_the_column() {
        // Down/Up step to the cell above/below in the *same column*, not back to
        // the top-left the way a naive row/col motion over the picture would.
        let mut d = wysiwyg_doc("tbl_vert", TABLE);
        d.caret = TABLE.find("Qty").unwrap();
        // Each vertical hop selects the destination cell, holding the column.
        assert!(d.cell_move_vertical(true));
        assert_eq!(d.selected_text(), Some("3"), "Down holds column 1");
        assert!(d.cell_move_vertical(true));
        assert_eq!(d.selected_text(), Some("12"), "Down again, still column 1");
        assert!(!d.cell_move_vertical(true), "no row below the last");
        assert!(d.cell_move_vertical(false));
        assert_eq!(d.selected_text(), Some("3"), "Up holds column 1");
        assert!(d.cell_move_vertical(false));
        assert_eq!(d.selected_text(), Some("Qty"), "Up onto the header");
        assert!(!d.cell_move_vertical(false), "no row above the header");
    }

    #[test]
    fn tab_off_the_last_cell_grows_a_row_and_enters_it() {
        let mut d = wysiwyg_doc("tbl_grow", TABLE);
        d.caret = TABLE.rfind("12").unwrap();
        let rows_before = d.source.matches('\n').count();
        assert!(d.cell_tab(true), "acts as a table key");
        assert_eq!(
            d.source.matches('\n').count(),
            rows_before + 1,
            "a fresh row was appended"
        );
        assert!(d.caret_in_table(), "the caret entered the new row");
        // The caret sits in the new row's first cell — past the old last cell.
        assert!(d.caret > TABLE.rfind("12").unwrap());
    }

    #[test]
    fn return_in_a_table_drops_a_cell_and_grows_a_row_at_the_bottom() {
        let mut d = wysiwyg_doc("tbl_ret", TABLE);
        d.caret = TABLE.find("Name").unwrap();
        assert!(d.cell_return(), "acts as a table key");
        assert_eq!(
            d.selected_text(),
            Some("Pear"),
            "Return drops one cell, selecting it"
        );
        // From the last row, Return appends a row and enters it.
        d.caret = TABLE.rfind("Fig").unwrap();
        let rows_before = d.source.matches('\n').count();
        assert!(d.cell_return());
        assert_eq!(d.source.matches('\n').count(), rows_before + 1);
        assert!(d.caret_in_table());
    }

    #[test]
    fn return_and_tab_outside_a_table_decline() {
        let mut d = wysiwyg_doc("tbl_decline", "just a paragraph\n");
        d.caret = 4;
        assert!(!d.cell_return(), "no table: the frontend inserts a newline");
        assert!(!d.cell_tab(true), "no table: the frontend indents");
        assert!(
            !d.cell_line_break(),
            "no table: the frontend breaks the line"
        );
    }

    #[test]
    fn a_click_under_a_trailing_table_lands_past_it_and_enter_opens_a_line() {
        // A document that ends in a table used to end *inside* it: nothing
        // past the last cell was a caret stop, so a click in the blank space
        // under the grid snapped back into the table and there was no way to
        // write a line after it. The bottom border's end is that stop now.
        let mut d = wysiwyg_doc("tbl_trail", TABLE);
        let rows = d.vmap.num_rows();
        d.click(rows + 3, 0, false);
        let end = TABLE.trim_end_matches('\n').len();
        assert_eq!(d.caret, end, "the caret stands just past the table");
        assert!(!d.caret_in_table(), "past the table is outside it");
        assert!(!d.cell_return(), "Return there is the frontend's newline");
        d.newline();
        d.insert("after");
        assert_eq!(
            d.source,
            format!("{TABLE}\nafter\n"),
            "Enter opens a paragraph under the table"
        );
    }

    #[test]
    fn typing_at_a_table_s_trailing_stop_opens_a_paragraph_first() {
        // The stop sits at the end of the table's last source line, and a
        // line glued under a table is a row of it — `| Fig | 12 |x` would be a
        // three-cell row. So the text gets a paragraph of its own, as it does
        // beside a block picture.
        let mut d = wysiwyg_doc("tbl_type", TABLE);
        d.caret = TABLE.trim_end_matches('\n').len();
        d.insert("x");
        assert_eq!(d.source, format!("{TABLE}\nx\n"));
        assert_eq!(d.caret, TABLE.len() + 2, "the caret follows the text");
        // And a paste, which joins the block exactly as typing would.
        let mut d = wysiwyg_doc("tbl_paste", TABLE);
        d.caret = TABLE.trim_end_matches('\n').len();
        d.paste("pasted");
        assert_eq!(d.source, format!("{TABLE}\npasted\n"));
    }

    #[test]
    fn right_leaves_a_table_by_its_trailing_stop_and_backspace_steps_back_in() {
        let mut d = wysiwyg_doc("tbl_edge", TABLE);
        let last_cell_end = TABLE.rfind("12").unwrap() + 2;
        let end = TABLE.trim_end_matches('\n').len();
        d.caret = last_cell_end;
        d.move_right(false);
        assert_eq!(d.caret, end, "Right from the last cell leaves the table");
        // Backspace there takes no byte: the one behind the caret is the row's
        // closing `|`, which the rich view never drew. It steps back instead.
        d.backspace();
        assert_eq!(d.source, TABLE, "nothing deleted");
        assert_eq!(d.caret, last_cell_end, "back into the last cell");
        // Down from the last row lands on the same stop, and Up returns.
        d.move_down(false);
        assert_eq!(d.caret, end, "Down from the last row leaves the table");
        d.move_up(false);
        assert_eq!(d.caret, last_cell_end);
    }

    #[test]
    fn a_table_s_trailing_stop_sits_between_it_and_the_text_below() {
        // With prose under the table, the stop is one hop between the last
        // cell and the paragraph — the shape a block picture's second stop has.
        let src = format!("{TABLE}\nafter\n");
        let mut d = wysiwyg_doc("tbl_mid", &src);
        d.caret = TABLE.rfind("12").unwrap() + 2;
        d.move_right(false);
        assert_eq!(d.caret, TABLE.trim_end_matches('\n').len());
        d.move_right(false);
        assert_eq!(d.caret, src.find("after").unwrap());
        // Typing at the stop still opens a paragraph, and the text below keeps
        // its own.
        d.move_left(false);
        d.insert("x");
        assert_eq!(d.source, format!("{TABLE}\nx\n\nafter\n"));
    }

    #[test]
    fn shift_return_inserts_an_in_cell_break_the_renderer_reads_as_a_line() {
        let mut d = wysiwyg_doc("tbl_break", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4; // just after "Pear"
        assert!(d.cell_line_break(), "acts as a table key");
        assert!(
            d.source.contains("Pear<br>"),
            "spelled as an inline <br>: {}",
            d.source
        );
        assert!(d.caret_in_table(), "still in the cell, past the break");
        // The break renders as a real line: the "Pear" cell now draws two lines,
        // so the table's picture is one row taller than a single-line table.
        d.build_visual(80);
        let table = &d.vmap.tables[0];
        let cell = &table.grid[1].cells[0]; // first body row, first column
        assert!(
            cell.glyphs.iter().any(|g| g.ch == '\n'),
            "the cell carries the break as a newline glyph for the frontend to split"
        );
    }

    #[test]
    fn shift_return_in_a_markdown_cell_leaves_a_semantic_hard_break_not_raw_html() {
        // twig promotes the in-cell `<br>` to a `hard_break`, so the break reads
        // back as structure — the whole point of routing through insert_line_break
        // instead of splicing raw `<br>` bytes.
        let mut d = wysiwyg_doc("tbl_break_semantic", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        assert!(d.cell_line_break());
        let kinds: Vec<Kind> = d
            .editor
            .nodes()
            .unwrap()
            .iter()
            .map(|n| n.kind.clone())
            .collect();
        assert!(kinds.contains(&Kind::HardBreak), "got {kinds:?}");
        assert!(
            !kinds.contains(&Kind::RawInline),
            "still raw HTML: {kinds:?}"
        );
    }

    #[test]
    fn backspace_over_an_in_cell_break_deletes_the_whole_br_not_a_byte() {
        // The `<br>` draws as one newline glyph, so Backspace over it must take
        // all four bytes — a one-byte delete would strand a visible `<br` in the
        // cell (the reported bug).
        let mut d = wysiwyg_doc("tbl_break_bs", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        assert!(d.cell_line_break());
        assert!(d.source.contains("Pear<br>"), "precondition: {}", d.source);
        d.backspace(); // caret sits just past the break
        assert!(
            !d.source.contains("<br"),
            "no half-deleted <br left: {}",
            d.source
        );
        assert!(
            d.source.contains("| Pear |"),
            "the cell is back to one line: {}",
            d.source
        );
    }

    #[test]
    fn backspace_at_a_cell_start_is_a_wall() {
        // The task's repro: two presses used to take the padding and then the
        // `|`, merging `d` into `c`'s cell and leaving the row a column short.
        let src = "| a | b |\n| - | - |\n| c | d |\n";
        let mut d = wysiwyg_doc("tbl_wall_bs", src);
        d.place_caret(src.find("d |").unwrap(), false);
        for _ in 0..3 {
            d.backspace();
        }
        assert_eq!(d.source, src);
        // Inside the padding too — right after the `|`, before the space. Set
        // directly: `place_caret` would snap it onto the stop past the space.
        d.caret = src.find("| d").unwrap() + 1;
        d.backspace();
        assert_eq!(d.source, src);
        // The row's first cell, and an empty one, are walls the same.
        d.place_caret(src.find("c |").unwrap(), false);
        d.backspace();
        assert_eq!(d.source, src);
        // A cell that opens on hidden markup: the wall is the first letter
        // drawn, not the `**` in front of it.
        let bold = "| a | b |\n| - | - |\n| c | **d** |\n";
        let mut d = wysiwyg_doc("tbl_wall_bs_bold", bold);
        d.place_caret(bold.find("d**").unwrap(), false);
        d.backspace();
        assert_eq!(d.source, bold);
        let empty = "| a | b |\n| - | - |\n| c |   |\n";
        let mut d = wysiwyg_doc("tbl_wall_bs_empty", empty);
        d.place_caret(empty.find("|   |").unwrap() + 2, false);
        d.backspace();
        assert_eq!(d.source, empty);
    }

    #[test]
    fn backspace_inside_a_cell_still_deletes_a_character() {
        let src = "| a | b |\n| - | - |\n| c | de |\n";
        let mut d = wysiwyg_doc("tbl_wall_bs_mid", src);
        d.place_caret(src.find("e |").unwrap(), false);
        d.backspace();
        assert_eq!(d.source, "| a | b |\n| - | - |\n| c | e |\n");
    }

    #[test]
    fn delete_at_a_cell_end_is_a_wall() {
        // The mirror: Delete at `c`'s end would take the padding, then the `|`.
        let src = "| a | b |\n| - | - |\n| c | d |\n";
        let mut d = wysiwyg_doc("tbl_wall_del", src);
        d.place_caret(src.find("c |").unwrap() + 1, false);
        for _ in 0..3 {
            d.delete_forward();
        }
        assert_eq!(d.source, src);
        // And inside the trailing padding, set directly past the snap.
        d.caret = src.find("c |").unwrap() + 2;
        d.delete_forward();
        assert_eq!(d.source, src);
        // The row's last cell is a wall on its closing `|` as well.
        d.place_caret(src.find("d |").unwrap() + 1, false);
        d.delete_forward();
        assert_eq!(d.source, src);
        // Mid-cell Delete is untouched.
        d.place_caret(src.find("c |").unwrap(), false);
        d.delete_forward();
        assert_eq!(d.source, "| a | b |\n| - | - |\n|  | d |\n");
    }

    #[test]
    fn delete_forward_over_an_in_cell_break_deletes_the_whole_br() {
        let mut d = wysiwyg_doc("tbl_break_del", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        assert!(d.cell_line_break());
        d.caret = TABLE.find("Pear").unwrap() + 4; // back onto the break's start
        d.delete_forward();
        assert!(
            !d.source.contains("<br"),
            "no half-deleted <br: {}",
            d.source
        );
        assert!(
            d.source.contains("| Pear |"),
            "cell back to one line: {}",
            d.source
        );
    }

    #[test]
    fn shift_return_in_a_djot_cell_is_swallowed_and_leaves_the_row_intact() {
        // Djot has no idiomatic in-cell break, so twig refuses it. The gesture is
        // still consumed (a real newline would split the one-line row), but the
        // cell must be left exactly as it was — no non-idiomatic `<br>` spliced in.
        let src = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n";
        let mut d = Doc::from_source(src.to_string(), Format::Djot).unwrap();
        d.caret = src.find("Pear").unwrap() + 4;
        assert!(d.caret_in_table(), "caret should be inside the djot table");
        assert!(
            d.cell_line_break(),
            "the key is consumed, not passed to the frontend"
        );
        assert_eq!(d.source, src, "the djot cell is left untouched");
        assert!(
            !d.source.contains("<br>"),
            "no non-idiomatic <br> spliced into djot"
        );
        assert!(
            d.status.is_some(),
            "the refusal is surfaced on the status line"
        );
    }

    #[test]
    fn typing_in_a_cell_edits_that_cell() {
        // Editing comes free once offsets map correctly: the caret is a source
        // offset, so a normal splice lands inside the pipe table.
        let mut d = wysiwyg_doc("tbl_type", TABLE);
        d.caret = TABLE.find("Pear").unwrap() + 4;
        d.insert("s");
        assert!(d.source.contains("| Pears | 3 |"), "got {:?}", d.source);
    }

    #[test]
    fn motion_and_delete_treat_an_emoji_as_one_character() {
        // 👨‍👩‍👧 is a single grapheme built from three emoji joined by ZWJ — 18
        // bytes, several codepoints. Right-arrow must clear it in one step, and
        // backspace must remove the whole cluster, not a stray joiner.
        let family = "👨‍👩‍👧";
        let mut d = doc_with("emoji", &format!("a{family}b\n"));
        d.caret = 1; // just after 'a', before the emoji
        d.move_right(false);
        assert_eq!(
            d.caret,
            1 + family.len(),
            "one step clears the whole cluster"
        );
        assert_eq!(&d.source[d.caret..d.caret + 1], "b");

        d.backspace(); // delete the emoji as a unit
        assert_eq!(d.source, "ab\n");
        assert_eq!(d.caret, 1);
    }

    #[test]
    fn motion_handles_a_combining_accent_as_one_character() {
        // "e" + U+0301 (combining acute) renders as one é.
        let mut d = doc_with("combining", "e\u{0301}x\n");
        d.caret = 0;
        d.move_right(false);
        assert_eq!(
            d.caret,
            "e\u{0301}".len(),
            "steps past base + combining mark"
        );
    }

    #[test]
    fn undo_then_redo_round_trips_an_edit() {
        let mut d = doc_with("undo", "hello\n");
        d.caret = 5;
        d.insert("!");
        assert_eq!(d.source, "hello!\n");
        d.undo();
        assert_eq!(d.source, "hello\n");
        assert_eq!(d.caret, 5, "undo restores the caret");
        d.redo();
        assert_eq!(d.source, "hello!\n");
    }

    #[test]
    fn a_run_of_typing_undoes_as_one_step() {
        let mut d = doc_with("coalesce", "\n");
        d.caret = 0;
        d.insert("a");
        d.insert("b");
        d.insert("c");
        assert_eq!(d.source, "abc\n");
        d.undo(); // the whole typed run, not just "c"
        assert_eq!(d.source, "\n");
        d.undo(); // nothing left — the run was one step
        assert_eq!(d.source, "\n");
        assert_eq!(d.status.as_deref(), Some("nothing to undo"));
    }

    // ── IME composition ──────────────────────────────────────────────────────

    #[test]
    fn a_composition_run_undoes_as_one_step() {
        let mut d = doc_with("compose", "\n");
        d.caret = 0;
        // What an IME does: each step replaces the last one's provisional bytes.
        d.edit_composing(0, 0, "k");
        d.edit_composing(0, 1, "か");
        d.edit_composing(0, 3, "かん");
        d.edit_composing(0, 6, "感"); // the commit
        d.end_composition();
        assert_eq!(d.source, "感\n");
        d.undo(); // the whole composition, not its last keystroke
        assert_eq!(d.source, "\n");
        assert_eq!(d.status.as_deref(), None, "the run was a single step");
    }

    /// A Replace All the way a host does one: every match, last to first so
    /// the offsets still to go stay put, each a selection replaced by an
    /// insert, inside one undo group. `select_range` rather than the host's
    /// snapping `place_caret`, which would snap against a map these tests do
    /// not rebuild between edits.
    fn replace_all_in(d: &mut Doc, needle: &str, with: &str) {
        let hits: Vec<usize> = d.source.match_indices(needle).map(|(i, _)| i).collect();
        d.begin_undo_group();
        for at in hits.into_iter().rev() {
            d.select_range(at, at + needle.len());
            d.insert(with);
        }
        d.end_undo_group();
    }

    #[test]
    fn a_replace_all_undoes_and_redoes_as_one_step() {
        let mut d = wysiwyg_doc("group", "a cat, a **cat**, a cat\n");
        d.caret = 0;
        d.insert("x");
        replace_all_in(&mut d, "cat", "dog");
        assert_eq!(d.source, "xa dog, a **dog**, a dog\n");
        assert!(d.undo(), "one undo");
        assert_eq!(
            d.source, "xa cat, a **cat**, a cat\n",
            "puts back every match"
        );
        assert!(d.redo(), "one redo");
        assert_eq!(
            d.source, "xa dog, a **dog**, a dog\n",
            "replaces them all again"
        );
        assert!(d.undo());
        assert!(d.undo(), "the typing before is its own step");
        assert_eq!(d.source, "a cat, a **cat**, a cat\n");
        assert!(!d.can_undo());
    }

    #[test]
    fn typing_after_a_group_is_a_step_of_its_own() {
        // A one-character replacement is an insert like a keystroke, and a
        // keystroke after it would coalesce with it outside a group.
        let mut d = wysiwyg_doc("group", "a b a\n");
        replace_all_in(&mut d, "a", "c");
        d.caret = d.source.len() - 1;
        d.insert("d");
        assert_eq!(d.source, "c b cd\n");
        assert!(d.undo());
        assert_eq!(d.source, "c b c\n", "the typing alone");
        assert!(d.undo());
        assert_eq!(d.source, "a b a\n", "then the replacement, whole");
    }

    #[test]
    fn a_group_of_more_edits_than_the_history_holds_is_still_one_step() {
        // twig keeps 200 steps; a group folds as it goes, so it never holds
        // more than one of them.
        let src = format!("start\n{}\n", "x ".repeat(300));
        let mut d = wysiwyg_doc("group", &src);
        d.caret = 5;
        d.insert("!");
        replace_all_in(&mut d, "x", "yy");
        assert_eq!(d.undo_steps, 2);
        assert!(d.undo());
        assert_eq!(d.source, src.replacen("start", "start!", 1));
        assert!(d.undo());
        assert_eq!(d.source, src);
        assert!(!d.can_undo());
    }

    #[test]
    fn an_undo_closes_an_open_group() {
        let mut d = wysiwyg_doc("group", "one\n");
        d.caret = 3;
        d.begin_undo_group();
        d.begin_undo_group();
        d.insert("x");
        assert!(d.in_undo_group());
        assert!(d.undo());
        assert!(
            !d.in_undo_group(),
            "the history moved, so the group is over"
        );
        d.end_undo_group();
        d.end_undo_group();
        assert_eq!(d.source, "one\n");
        assert!(d.redo());
        assert_eq!(d.source, "onex\n");
    }

    #[test]
    fn replacing_the_source_keeps_the_caret_on_its_characters() {
        let mut d = wysiwyg_doc("replace", "One.\n\nTwo.\n\nThree.\n");
        let on = d.source.find("Three").unwrap() + 2;
        d.caret = on;
        // Another device's edit, before the caret: the caret moves along.
        assert!(d.replace_source("One, and more.\n\nTwo.\n\nThree.\n"));
        assert_eq!(d.source, "One, and more.\n\nTwo.\n\nThree.\n");
        assert_eq!(&d.source[d.caret..], "ree.\n", "still inside Three");
        // After the caret: it stays.
        let at = d.caret;
        assert!(d.replace_source("One, and more.\n\nTwo.\n\nThree.\n\nFour.\n"));
        assert_eq!(d.caret, at);
        assert!(d.selection().is_none());
        // One undo step, the caret where it stood.
        assert!(d.undo());
        assert_eq!(d.source, "One, and more.\n\nTwo.\n\nThree.\n");
        assert_eq!(d.caret, at);
    }

    #[test]
    fn replacing_the_source_writes_markup_as_markup() {
        let mut d = wysiwyg_doc("replace-markup", "plain\n");
        assert!(d.replace_source("**bold** and *it*\n"));
        assert_eq!(
            d.source, "**bold** and *it*\n",
            "source, not typing: nothing escaped"
        );
        assert!(d.replace_source("**bold** and *it*\n"), "already there");
    }

    #[test]
    fn the_differing_span_ends_on_characters() {
        assert_eq!(differing_span("abc", "abc"), (3, 3, ""));
        assert_eq!(differing_span("abXc", "abYc"), (2, 3, "Y"));
        assert_eq!(differing_span("aaa", "aaaa"), (3, 3, "a"));
        assert_eq!(differing_span("aaaa", "aa"), (2, 4, ""));
        // é and è share their first byte; the span must not split it.
        assert_eq!(differing_span("café", "cafè"), (3, 5, "è"));
        assert_eq!(differing_span("", "x"), (0, 0, "x"));
    }

    #[test]
    fn a_substitution_behind_the_caret_leaves_the_caret_and_is_its_own_step() {
        let mut d = wysiwyg_doc("substitute", "\n");
        d.caret = 0;
        for c in ["I", " ", "s", "a", "w", " ", "t", "e", "h", " "] {
            d.insert(c);
        }
        assert_eq!(d.source, "I saw teh \n");
        let at = d.source.find("teh").unwrap();
        assert!(d.substitute(at, at + 3, "the"));
        assert_eq!(d.source, "I saw the \n");
        assert_eq!(d.caret, 10, "still after the space");
        assert!(d.selection().is_none());
        d.insert("c");
        assert_eq!(d.source, "I saw the c\n");
        assert!(d.undo(), "the typing after it is a step of its own");
        assert_eq!(d.source, "I saw the \n");
        assert!(d.undo(), "the correction is one step");
        assert_eq!(d.source, "I saw teh \n", "what was typed");
        assert_eq!(d.caret, 10, "with the caret where it stood");
        assert!(d.redo());
        assert_eq!(d.source, "I saw the \n");
        assert!(d.undo());
        assert!(d.undo(), "and the typing before it is its own");
        assert_eq!(d.source, "\n");
        assert!(!d.can_undo());
    }

    #[test]
    fn a_substitution_keeps_the_markup_it_stands_beside() {
        // A quote typed just past a bold run: only its own byte changes.
        let mut d = wysiwyg_doc("substitute", "A **bold** word\n");
        let at = d.source.find(" word").unwrap();
        d.caret = at;
        d.insert("\"");
        assert_eq!(d.source, "A **bold**\" word\n");
        assert!(d.substitute(at, at + 1, "\u{201d}"));
        assert_eq!(d.source, "A **bold**\u{201d} word\n");
        assert_eq!(d.caret, at + "\u{201d}".len());
        assert!(
            d.marks_at(d.source.find("bold").unwrap())
                .iter()
                .any(|(k, _)| *k == InlineKind::Strong)
        );
        // A range that is not one is refused, and changes nothing.
        let src = d.source.clone();
        assert!(!d.substitute(3, 2, "x"));
        assert!(!d.substitute(0, src.len() + 1, "x"));
        assert!(!d.substitute(d.source.find('\u{201d}').unwrap() + 1, d.source.len(), "x"));
        assert_eq!(d.source, src);
    }

    #[test]
    fn a_substitution_that_half_lands_takes_back_only_its_own_deletion() {
        // The deletion lands and twig refuses the literal text: the deleted
        // bytes go back, and nothing else moves — not an earlier substitution
        // of the same keystroke, not the group it is in, and no redo is left
        // to delete them again.
        let mut d = wysiwyg_doc("substitute", "\n");
        d.set_markup_mode(MarkupMode::None);
        d.caret = 0;
        for c in ["a", "\"", "b", " ", "\""] {
            d.insert(c);
        }
        assert_eq!(d.source, "a\"b \"\n");

        d.refuse_next_literal = true;
        assert!(!d.substitute(1, 2, "\u{201c}"));
        assert_eq!(d.source, "a\"b \"\n");
        assert_eq!(d.caret, 5, "the caret where it stood");
        assert!(!d.can_redo(), "no redo step left behind");

        d.begin_undo_group();
        assert!(d.substitute(4, 5, "\u{201d}"));
        d.refuse_next_literal = true;
        assert!(!d.substitute(1, 2, "\u{201c}"));
        assert!(d.in_undo_group(), "the group is still open");
        assert_eq!(d.source, "a\"b \u{201d}\n", "the first substitution stands");
        d.end_undo_group();
        assert!(!d.can_redo());
        assert!(d.undo());
        assert_eq!(d.source, "a\"b \"\n", "one step takes the group back");
        assert!(d.undo());
        assert_eq!(d.source, "\n", "and the typing is still one step");
        assert!(!d.can_undo());

        // A group whose first edit is the one that half lands has no step.
        let mut d = wysiwyg_doc("substitute", "\n");
        d.set_markup_mode(MarkupMode::None);
        d.caret = 0;
        d.insert("\"");
        d.begin_undo_group();
        d.refuse_next_literal = true;
        assert!(!d.substitute(0, 1, "\u{201c}"));
        assert!(d.substitute(0, 1, "\u{201c}"));
        d.end_undo_group();
        assert!(d.undo());
        assert_eq!(
            d.source, "\"\n",
            "the substitution that landed is the group's step"
        );
        assert!(d.undo());
        assert_eq!(d.source, "\n");
    }

    #[test]
    fn a_substitution_is_written_the_way_typing_writes() {
        // Where typing is literal, so is a replacement's text.
        let mut d = wysiwyg_doc("substitute", "say hi\n");
        d.set_markup_mode(MarkupMode::None);
        assert!(d.substitute(4, 6, "*hi*"));
        assert_eq!(d.source, "say \\*hi\\*\n");
        assert!(d.undo());
        assert_eq!(d.source, "say hi\n", "one step, escapes and all");
    }

    #[test]
    fn can_undo_is_false_once_a_coalesced_run_is_undone() {
        // The task's repro: five characters typed are one twig step, and the
        // counter used to say five.
        let mut d = wysiwyg_doc("undo_truth", "\n");
        d.caret = 0;
        for c in ["h", "e", "l", "l", "o"] {
            d.insert(c);
        }
        assert!(d.can_undo());
        assert!(d.undo(), "the run undoes");
        assert_eq!(d.source, "\n");
        assert!(!d.can_undo(), "and nothing is left behind it");
        let rev = d.revision();
        assert!(!d.undo(), "an undo that does not move says so");
        assert_eq!(d.revision(), rev);
        assert!(d.can_redo());
        assert!(d.redo());
        assert_eq!(d.source, "hello\n");
        assert!(!d.can_redo());
        assert!(!d.redo());
    }

    #[test]
    fn can_undo_is_exact_across_the_gestures_that_coalesce() {
        // Each gesture below folds, or may fold, one twig step into another.
        // After it, the mirror must name exactly the number of undos twig
        // takes — and the same number of redos back.
        type Gesture = fn(&mut Doc);
        let cases: &[(&str, &str, Gesture)] = &[
            ("typing", "\n", |d| {
                d.caret = 0;
                d.insert("ab");
                d.insert("c");
                d.move_left(false);
                d.insert("x");
            }),
            ("backspaces", "abcdef\n", |d| {
                d.caret = 6;
                d.backspace();
                d.backspace();
                d.insert("z");
                d.backspace();
            }),
            ("ordered list item", "1. one\n2. two\n", |d| {
                d.caret = 6;
                d.newline();
                d.insert("x");
            }),
            ("list indent", "- one\n- two\n", |d| {
                d.caret = d.source.find("two").unwrap();
                d.indent();
                d.outdent();
            }),
            ("bold then type", "one two\n", |d| {
                d.select_range(0, 3);
                d.toggle(InlineKind::Strong);
                d.caret = d.source.len() - 1;
                d.insert("!");
            }),
            ("highlight", "one two\n", |d| {
                d.select_range(0, 3);
                d.highlight(None);
                d.highlight(Some(MarkColor::Green));
            }),
            ("heading and quote", "one\n", |d| {
                d.caret = 1;
                d.toggle_heading(2);
                d.toggle_blockquote();
            }),
            ("footnote", "one\n", |d| {
                d.caret = 3;
                d.insert_footnote();
                d.insert("note");
            }),
            ("blocks", "one\n\ntwo\n\nthree\n", |d| {
                d.caret = 1;
                d.move_block_down();
                d.toggle_list(true);
                d.set_alignment(Some(Align::Center));
                d.insert_page_break();
                d.insert_table(2, 2);
            }),
            ("paste and compose", "\n", |d| {
                d.caret = 0;
                d.paste("one two");
                d.delete_word_back();
                d.edit_composing(d.caret, d.caret, "か");
                d.edit_composing(d.caret - 3, d.caret, "蚊");
                d.end_composition();
                d.insert("z");
            }),
            ("table", "| a | b |\n| - | - |\n| c | d |\n", |d| {
                d.place_caret(d.source.find("d |").unwrap(), false);
                d.cell_tab(true);
                d.insert("e");
                d.cell_return();
            }),
            ("replace all", "cat cat cat\n", |d| {
                d.caret = 0;
                d.insert("a ");
                replace_all_in(d, "cat", "dog");
                d.insert("!");
            }),
            ("nested group", "one\n\ntwo\n", |d| {
                d.begin_undo_group();
                d.caret = 3;
                d.insert("x");
                d.begin_undo_group();
                d.toggle_heading(1);
                d.end_undo_group();
                d.insert("y");
                d.end_undo_group();
                d.backspace();
            }),
            ("group around a list", "1. one\n2. two\n", |d| {
                d.begin_undo_group();
                d.caret = 6;
                d.newline();
                d.insert("x");
                d.end_undo_group();
            }),
            ("substitution", "\n", |d| {
                d.caret = 0;
                d.insert("t");
                d.insert("e");
                d.insert("h");
                d.insert(" ");
                d.substitute(0, 3, "the");
                d.insert("c");
            }),
            ("empty group", "one\n", |d| {
                d.caret = 3;
                d.insert("x");
                d.begin_undo_group();
                d.end_undo_group();
                d.insert("y");
            }),
            ("undo inside a group", "one\n", |d| {
                d.caret = 3;
                d.begin_undo_group();
                d.insert("x");
                d.undo();
                d.insert("y");
                d.insert("z");
                d.end_undo_group();
            }),
        ];
        for (name, src, gesture) in cases {
            let mut d = wysiwyg_doc("undo_exact", src);
            gesture(&mut d);
            let (steps, after) = (d.undo_steps, d.source.clone());
            let mut undone = 0;
            while d.can_undo() {
                assert!(d.undo(), "{name}: can_undo said yes, undo moved nothing");
                undone += 1;
            }
            assert!(!d.undo(), "{name}: can_undo said no, undo moved");
            assert_eq!(undone, steps, "{name}");
            assert_eq!(d.source, *src, "{name}: back to the start");
            let mut redone = 0;
            while d.can_redo() {
                assert!(d.redo(), "{name}: can_redo said yes, redo moved nothing");
                redone += 1;
            }
            assert!(!d.redo(), "{name}: can_redo said no, redo moved");
            assert_eq!(redone, steps, "{name}");
            assert_eq!(d.source, after, "{name}: forward to the end");
        }
    }

    #[test]
    fn two_compositions_are_two_undo_steps() {
        let mut d = doc_with("compose_two", "\n");
        d.caret = 0;
        d.edit_composing(0, 0, "か");
        d.edit_composing(0, 3, "蚊");
        d.end_composition();
        d.edit_composing(3, 3, "き");
        d.edit_composing(3, 6, "木");
        d.end_composition();
        assert_eq!(d.source, "蚊木\n");
        d.undo();
        assert_eq!(d.source, "蚊\n", "only the second composition");
        d.undo();
        assert_eq!(d.source, "\n");
    }

    #[test]
    fn a_composition_does_not_fold_into_the_typing_around_it() {
        let mut d = doc_with("compose_typing", "\n");
        d.caret = 0;
        d.insert("a");
        d.insert("b");
        d.edit_composing(2, 2, "か");
        d.edit_composing(2, 5, "蚊");
        d.end_composition();
        d.insert("c");
        assert_eq!(d.source, "ab蚊c\n");
        d.undo();
        assert_eq!(d.source, "ab蚊\n");
        d.undo();
        assert_eq!(d.source, "ab\n");
        d.undo();
        assert_eq!(d.source, "\n");
    }

    #[test]
    fn ending_a_composition_that_never_began_leaves_a_typing_run_alone() {
        let mut d = doc_with("compose_spurious", "\n");
        d.caret = 0;
        d.insert("a");
        d.end_composition(); // an IME unmarking unprompted
        d.insert("b");
        assert_eq!(d.source, "ab\n");
        d.undo();
        assert_eq!(d.source, "\n", "still one typed run");
    }

    // ── the clipboard's rich flavor ──────────────────────────────────────────

    #[test]
    fn an_inline_selection_publishes_html_without_a_paragraph_wrapper() {
        let mut d = doc_with("sel_inline", "a **bold** c\n");
        d.anchor = Some(2);
        d.caret = 10; // `**bold**`, inside the paragraph
        assert_eq!(d.selection_html().as_deref(), Some("<strong>bold</strong>"));
    }

    #[test]
    fn a_whole_block_selection_keeps_its_paragraph() {
        let mut d = doc_with("sel_block", "a **bold** c\n");
        d.anchor = Some(0);
        d.caret = 12; // the entire paragraph
        assert_eq!(
            d.selection_html().as_deref(),
            Some("<p>a <strong>bold</strong> c</p>")
        );
    }

    #[test]
    fn a_multi_block_selection_keeps_its_structure() {
        let mut d = doc_with("sel_multi", "para\n\n- one\n- two\n");
        d.select_all();
        let html = d.selection_html().expect("renders");
        assert!(html.contains("<p>para</p>"), "{html:?}");
        assert!(html.contains("<li>one</li>"), "{html:?}");
    }

    #[test]
    fn a_word_inside_a_heading_publishes_as_text_not_a_heading() {
        // The fragment `Head` is a paragraph standalone; the *document* says it
        // sits inside one block, so the wrapper is an artifact either way.
        let mut d = doc_with("sel_heading", "# Head line\n");
        d.anchor = Some(2);
        d.caret = 6;
        assert_eq!(d.selection_html().as_deref(), Some("Head"));
    }

    #[test]
    fn no_selection_publishes_no_html() {
        let mut d = doc_with("sel_none", "a b\n");
        d.caret = 1;
        assert_eq!(d.selection_html(), None);
    }

    #[test]
    fn pasting_html_converts_it_and_is_one_undo_step() {
        let mut d = doc_with("paste_html", "x\n");
        d.caret = 1;
        assert!(d.paste_html("<p>a <strong>b</strong> c</p>"));
        assert_eq!(d.source, "xa **b** c\n");
        d.undo();
        assert_eq!(d.source, "x\n", "the whole paste, in one step");
    }

    #[test]
    fn pasting_html_replaces_the_selection() {
        let mut d = doc_with("paste_html_sel", "keep drop\n");
        d.anchor = Some(5);
        d.caret = 9;
        assert!(d.paste_html("<em>new</em>"));
        assert_eq!(d.source, "keep *new*\n");
    }

    #[test]
    fn html_that_would_paste_garbage_declines_so_the_caller_falls_back() {
        let mut d = doc_with("paste_html_bad", "x\n");
        d.caret = 1;
        // twig builds no table from HTML; raw `<table>` in prose is worse than
        // the plain flavor the caller still holds.
        assert!(!d.paste_html("<table><tr><td>a</td></tr></table>"));
        assert_eq!(d.source, "x\n", "declined edits nothing");
    }

    #[test]
    fn copy_then_paste_round_trips_through_the_html_flavor() {
        let mut d = doc_with("clip_round", "a **b** and [l](https://x.dev)\n");
        d.select_all();
        let html = d.selection_html().expect("renders");
        let mut into = doc_with("clip_round_dst", "\n");
        into.caret = 0;
        assert!(into.paste_html(&html));
        assert_eq!(into.source, "a **b** and [l](https://x.dev)\n");
    }

    #[test]
    fn moving_the_caret_starts_a_new_undo_group() {
        let mut d = doc_with("break", "\n");
        d.caret = 0;
        d.insert("a");
        d.insert("b"); // "ab\n", caret at 2
        d.move_left(false); // breaks the run
        d.insert("X"); // "aXb\n"
        assert_eq!(d.source, "aXb\n");
        d.undo();
        assert_eq!(
            d.source, "ab\n",
            "first undo removes only the post-move insert"
        );
        d.undo();
        assert_eq!(d.source, "\n", "second undo removes the earlier run");
    }

    #[test]
    fn undo_reverses_a_format_toggle() {
        let mut d = doc_with("fmt_undo", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "a **word** b\n");
        d.undo();
        assert_eq!(d.source, "a word b\n");
    }

    #[test]
    fn undo_back_to_the_saved_state_clears_dirty() {
        let mut d = doc_with("dirty_undo", "hello\n");
        assert!(!d.dirty);
        d.caret = 5;
        d.insert("!");
        assert!(d.dirty);
        d.undo();
        assert!(
            !d.dirty,
            "undoing to the saved source is not a modification"
        );
    }

    #[test]
    fn marking_saved_as_what_was_written_keeps_later_typing_dirty() {
        let mut d = doc_with("saved_as", "Does this\n");
        d.caret = 9;
        d.insert(" ");
        // The host reads the source, and its write takes a while…
        let written = d.source.clone();
        // …during which the rest of the sentence is typed.
        d.insert("work?");
        d.mark_saved_as(&written);
        assert!(d.dirty, "what was typed during the write is not saved");
        d.undo();
        assert!(!d.dirty, "undoing to what was written is the saved state");

        d.redo();
        let written = d.source.clone();
        d.mark_saved_as(&written);
        assert!(!d.dirty, "nothing typed meanwhile: saved");
    }

    #[test]
    fn a_new_edit_invalidates_redo() {
        let mut d = doc_with("redo_inv", "\n");
        d.caret = 0;
        d.insert("a");
        d.undo();
        d.insert("b"); // diverges — the redo of "a" is now gone
        d.redo();
        assert_eq!(d.source, "b\n");
    }

    #[test]
    fn can_undo_and_can_redo_follow_the_history_a_menu_would_enable_by() {
        let mut d = doc_with("can_undo", "hello\n");
        assert!(
            !d.can_undo() && !d.can_redo(),
            "a fresh document has no history"
        );
        d.caret = 5;
        d.insert("!");
        assert!(
            d.can_undo() && !d.can_redo(),
            "an edit is a step to take back"
        );
        d.undo();
        assert!(!d.can_undo() && d.can_redo(), "undone: only redo remains");
        d.redo();
        assert!(d.can_undo() && !d.can_redo(), "redone: back to undoable");
        d.undo();
        d.insert("?");
        assert!(
            d.can_undo() && !d.can_redo(),
            "a fresh edit ends the redo chain"
        );
        // A coalesced run over-counts steps — the bound is what a menu needs,
        // and it reconciles the moment twig reports the history empty.
        d.insert("a");
        d.insert("b");
        while d.can_undo() {
            d.undo();
        }
        assert_eq!(d.source, "hello\n");
        assert!(!d.can_undo());
        // A reading surface has nothing to undo, whatever the history holds.
        d.redo();
        d.set_read_only(true);
        assert!(!d.can_undo() && !d.can_redo());
    }

    #[test]
    fn undo_on_empty_history_is_a_no_op() {
        let mut d = doc_with("undo_empty", "hi\n");
        d.undo();
        assert_eq!(d.source, "hi\n");
        assert_eq!(d.status.as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn a_one_character_paste_is_its_own_undo_step() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "paste_step", "ab\n");
            d.caret = 0;
            d.insert("x");
            d.insert("y"); // a run of typing
            d.paste("z"); // one character, but pasted — not part of that run
            assert_eq!(d.source, "xyzab\n");
            d.undo();
            assert_eq!(d.source, "xyab\n", "the paste undoes on its own");
            assert_eq!(d.caret, 2, "and hands back the caret it found");
            d.undo();
            assert_eq!(d.source, "ab\n", "the typed run is still one step under it");
        }
    }

    #[test]
    fn the_same_character_typed_still_joins_the_run() {
        // The other half of the pair: `z` is a keystroke here and a paste above,
        // and the two undo differently. Nothing about the *string* says which —
        // which is why provenance has to come from the door the caller uses.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "typed_run", "ab\n");
            d.caret = 0;
            d.insert("x");
            d.insert("y");
            d.insert("z");
            d.undo();
            assert_eq!(d.source, "ab\n", "one run, one step");
        }
    }

    #[test]
    fn undo_restores_the_caret_to_where_it_was_not_to_the_edit_site() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "undo_caret", "hello world\n");
            d.caret = 11; // standing at the end of "world", away from the edit
            d.edit(0, 5, "goodbye");
            assert_eq!(d.source, "goodbye world\n");
            d.undo();
            assert_eq!(d.source, "hello world\n");
            // The undone edit ends at offset 5; the user was at 11.
            assert_eq!(d.caret, 11, "the caret comes back with the bytes");
        }
    }

    #[test]
    fn undo_restores_the_selection_the_edit_replaced() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "undo_sel", "a word b\n");
            d.anchor = Some(2);
            d.caret = 6; // "word" selected
            d.insert("X");
            assert_eq!(d.source, "a X b\n");
            d.undo();
            assert_eq!(d.source, "a word b\n");
            assert_eq!(d.selection(), Some((2, 6)), "the selection comes back too");
        }
    }

    #[test]
    fn redo_restores_the_caret_the_edit_left_behind() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "redo_caret", "hello world\n");
            d.caret = 11;
            d.edit(0, 5, "goodbye");
            assert_eq!(d.caret, 7, "the edit left the caret after its new text");
            d.undo();
            d.redo();
            assert_eq!(d.source, "goodbye world\n");
            assert_eq!(d.caret, 7, "redo puts it back where the edit had it");
        }
    }

    #[test]
    fn undoing_a_typed_run_restores_the_caret_from_before_the_whole_run() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "run_caret", "hi\n");
            d.caret = 2;
            d.insert("a");
            d.insert("b");
            d.insert("c");
            assert_eq!(d.source, "hiabc\n");
            d.undo();
            assert_eq!(d.source, "hi\n");
            assert_eq!(d.caret, 2, "before the run, not before its last keystroke");
            d.redo();
            assert_eq!(d.caret, 5, "and redo restores the end of the whole run");
        }
    }

    #[test]
    fn undo_restores_the_caret_across_a_format_toggle() {
        // A toggle reaches twig without going through `splice`, so it has to
        // record its own step — miss it and every stack depth below it is off by
        // one, and undo starts handing back another edit's caret.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "fmt_caret", "a word b\n");
            d.caret = 8;
            d.anchor = Some(2);
            d.caret = 6;
            d.toggle(InlineKind::Strong);
            assert_eq!(d.source, "a **word** b\n");
            d.undo();
            assert_eq!(d.source, "a word b\n");
            assert_eq!(
                d.selection(),
                Some((2, 6)),
                "the toggled selection comes back"
            );
        }
    }

    #[test]
    fn an_edit_after_an_undo_truncates_the_caret_history_with_twigs() {
        // The drift that would never announce itself: twig drops its redo stack
        // on any fresh edit, so a leaf redo entry that outlives it would restore
        // a caret from the timeline that edit abandoned.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "redo_trunc", "hello world\n");
            d.caret = 11;
            d.edit(0, 5, "goodbye"); // step A, caret 11 → 7
            d.undo();
            assert_eq!(d.caret, 11);
            d.caret = 0;
            d.insert("X"); // diverges: A's redo is gone from twig
            assert_eq!(d.source, "Xhello world\n");

            d.redo();
            assert_eq!(d.source, "Xhello world\n", "nothing to redo onto");
            assert_eq!(d.status.as_deref(), Some("nothing to redo"));
            d.undo();
            assert_eq!(d.source, "hello world\n");
            assert_eq!(
                d.caret, 0,
                "the surviving step's caret, not the dropped one"
            );
        }
    }

    #[test]
    fn indent_and_outdent_move_the_caret_line_with_its_text() {
        for view in [View::Source, View::Wysiwyg] {
            let g = |m, f: fn(&mut Doc)| golden_in(view, "indent_line", m, f);
            assert_eq!(g("he|llo\n", |d| d.indent()), "  he|llo\n");
            assert_eq!(g("  he|llo\n", |d| d.outdent()), "he|llo\n");
            // Indentation the caret is standing *in* collapses to the line start
            // rather than dragging the caret into the text.
            assert_eq!(g("| hello\n", |d| d.outdent()), "|hello\n");
            // A line with none to give back is left exactly as it was.
            assert_eq!(g("he|llo\n", |d| d.outdent()), "he|llo\n");
            // Less than a full level gives back what it has.
            assert_eq!(g(" he|llo\n", |d| d.outdent()), "he|llo\n");
            // A tab is one level however many spaces it isn't.
            assert_eq!(g("\the|llo\n", |d| d.outdent()), "he|llo\n");
        }
    }

    #[test]
    fn one_indent_level_leaves_a_paragraph_a_paragraph() {
        // Why the level is two spaces and not the four both frontends type
        // today. Four is markdown's indented-code-block marker, so a Tab on a
        // paragraph would silently restyle it as code — a width that changes
        // what the document *means* isn't an indent. Pinned because the number
        // is the kind of thing a later list-aware pass would reach for.
        let mut d = doc_with("indent_kind", "hello\n");
        d.caret = 2;
        d.indent();
        assert_eq!(d.source, "  hello\n");
        assert!(
            d.nodes().iter().any(|n| n.kind == Kind::Para),
            "still prose after a Tab"
        );
        assert!(!d.nodes().iter().any(|n| n.kind == Kind::CodeBlock));

        // The four-space level this replaces, for contrast: same text, and twig
        // reparses the paragraph into a code block.
        let mut wide = doc_with("indent_kind_4", "    hello\n");
        wide.build_visual(80);
        assert!(
            wide.nodes().iter().any(|n| n.kind == Kind::CodeBlock),
            "four spaces is a code block, not an indented paragraph"
        );
    }

    #[test]
    fn indent_nests_a_list_item_under_its_parent() {
        // Tab indents a list item by its own marker width, landing its marker at
        // the parent's content column so twig reparses it as a nested list.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_nest", "- a\n- b\n");
            d.caret = 6; // on the second item
            d.indent();
            assert_eq!(d.source, "- a\n  - b\n");
            let lists = d
                .nodes()
                .iter()
                .filter(|n| n.kind == Kind::BulletList)
                .count();
            assert_eq!(lists, 2, "the indented item is a nested list");
        }
    }

    #[test]
    fn indent_nests_an_ordered_item_at_its_marker_width() {
        // An ordered marker `1. ` is three columns wide, so a two-space step
        // (which nests a bullet) leaves it flat. Regression: Tab must use the
        // marker width, three, so the item actually nests — and the source
        // renumbers so the sub-list restarts at 1 and the outer list resumes.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_ord", "1. a\n2. b\n3. c\n");
            d.caret = d.source.find('b').unwrap();
            d.indent();
            assert_eq!(d.source, "1. a\n   1. b\n2. c\n");
            let lists = d
                .nodes()
                .iter()
                .filter(|n| n.kind == Kind::OrderedList)
                .count();
            assert_eq!(lists, 2, "the indented item is a nested ordered list");
        }
    }

    #[test]
    fn indent_leaves_a_lists_first_item_put() {
        // The first item of a list has no sibling above it to nest under, so Tab
        // is a no-op there — the marker stays at column zero rather than being
        // shoved into indentation twig can't read as a sub-list.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_first", "- a\n- b\n");
            d.caret = 1; // on the FIRST item
            d.indent();
            assert_eq!(d.source, "- a\n- b\n", "the first item doesn't nest");
            // The sibling below still nests, proving the guard is per-item.
            d.caret = d.source.find('b').unwrap();
            d.indent();
            assert_eq!(d.source, "- a\n  - b\n");
        }
    }

    #[test]
    fn hidden_mode_keeps_typed_markup_literal() {
        // The Diaryx default: typing `*hi*` gives the characters, not emphasis —
        // twig escapes what would open markup, so the source is `\*hi\*` and the
        // AST is a plain string. Formatting is the commands' job in this mode.
        let mut d = doc_in(View::Wysiwyg, "hidden_literal", "");
        d.insert("*hi*");
        assert_eq!(d.source, "\\*hi\\*");
        assert!(
            d.nodes()
                .iter()
                .all(|n| n.kind != Kind::Emph && n.kind != Kind::Strong)
        );
    }

    #[test]
    fn hidden_mode_escapes_a_line_start_block_marker() {
        // A `#`/`-`/`>` at a line start would open a block, so Hidden mode keeps
        // it literal too — a Diaryx user's "# 1 idea" stays prose, not a heading.
        let mut d = doc_in(View::Wysiwyg, "hidden_block", "");
        d.insert("# hi");
        assert_eq!(d.source, "\\# hi");
        assert!(d.nodes().iter().all(|n| n.kind != Kind::Heading));
    }

    #[test]
    fn authoring_modes_keep_typed_markup_live() {
        // Both authoring rungs of the ladder: typing `*hi*` really is emphasis
        // (no escape), the same as source view — escaping is `None`'s alone, and
        // it's the axis, not the reveal, that decides.
        for (view, mode) in [
            (View::Wysiwyg, MarkupMode::Shortcuts),
            (View::Wysiwyg, MarkupMode::Full),
            (View::Source, MarkupMode::None),
        ] {
            let mut d = doc_in(view, "live_markup", "");
            d.set_markup_mode(mode);
            d.insert("*hi*");
            assert_eq!(d.source, "*hi*", "{mode:?} in {view:?} types raw markup");
        }
    }

    #[test]
    fn hidden_mode_overwrite_undoes_in_one_step() {
        // Typing over a selection escapes the replacement *and* stays a single
        // undo — the selection-delete and the literal insert fold together, so
        // one undo brings the whole selection back, like a plain overwrite.
        let mut d = doc_in(View::Wysiwyg, "hidden_overwrite", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6; // "word"
        d.insert("*");
        assert_eq!(d.source, "a \\* b\n", "the replacement is escaped");
        d.undo();
        assert_eq!(d.source, "a word b\n");
        assert_eq!(d.selection(), Some((2, 6)), "one undo, selection restored");
    }

    #[test]
    fn backspace_over_an_escaped_char_takes_the_hidden_backslash_too() {
        // Type `*` in Hidden mode → `\*` (drawn as one `*`); one Backspace clears
        // the whole visual character, never stranding the hidden `\`.
        let mut d = doc_in(View::Wysiwyg, "bsp_escape", "");
        d.insert("*");
        assert_eq!(d.source, "\\*");
        d.backspace();
        assert_eq!(d.source, "", "the escape backslash went with the *");
        // A *literal* backslash (source view, no escape) is an ordinary char.
        let mut s = doc_in(View::Source, "bsp_lit", "a\\b\n");
        s.caret = 3; // after `b`
        s.backspace();
        assert_eq!(s.source, "a\\\n", "only the b is deleted, the \\ stays");
    }

    #[test]
    fn hidden_mode_leaves_structural_markup_alone() {
        // Enter continues a bullet list by writing a real `- ` marker (an
        // `insert_raw`, not the typing path), so Hidden mode's escaping never
        // touches it — the list keeps working.
        let mut d = doc_in(View::Wysiwyg, "hidden_struct", "- item\n");
        d.caret = 6;
        d.newline();
        d.insert("two");
        assert_eq!(d.source, "- item\n- two\n");
    }

    #[test]
    fn markup_mode_defaults_to_none_and_round_trips() {
        // Diaryx's default is the clean `None` surface; a markup-fluent
        // frontend can climb the ladder, and the choice sticks.
        let mut d = doc_in(View::Wysiwyg, "markup_mode", "hi\n");
        assert_eq!(d.markup_mode(), MarkupMode::None, "None by default");
        for mode in [MarkupMode::Shortcuts, MarkupMode::Full, MarkupMode::None] {
            d.set_markup_mode(mode);
            assert_eq!(d.markup_mode(), mode);
        }
    }

    #[test]
    fn full_mode_reveals_only_the_caret_line() {
        // The mode's whole claim: the caret's line shows its raw delimiters and
        // every other line stays resolved. Two paragraphs with identical markup
        // so the only difference between the rows is where the caret is.
        let mut d = doc_in(
            View::Wysiwyg,
            "reveal_caret_line",
            "*one* here\n\n*two* there\n",
        );
        d.set_markup_mode(MarkupMode::Full);

        caret_at(&mut d, "one");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == "*one* here"),
            "caret's line raw: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r == "two there"),
            "other line resolved: {rows:?}"
        );

        // Move to the other paragraph: the reveal follows, and the line just
        // left goes back to being resolved.
        caret_at(&mut d, "two");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == "*two* there"),
            "caret's line raw: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r == "one here"),
            "left line resolved: {rows:?}"
        );
    }

    #[test]
    fn revealing_a_coloured_highlight_shows_the_emoji_that_spelled_it() {
        // The emoji is a delimiter, not content — so `MarkupMode::Full` owes it
        // the same treatment as an emphasis's `*`: hidden while the caret is
        // elsewhere, shown in full where the caret lands. That falls out of
        // `delims` reading the bytes between the mark's span and its content
        // span, which is exactly `==🔴 ` and `==`, rather than from a table
        // of spellings — so the no-space form `==🟢green==` reveals right too.
        let mut d = doc_in(
            View::Wysiwyg,
            "reveal_coloured_mark",
            "a ==🔴 red== one\n\nb ==plain== two\n",
        );
        d.set_markup_mode(MarkupMode::Full);

        caret_at(&mut d, "red");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == "a ==🔴 red== one"),
            "the caret's line shows the colour it was written with: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r == "b plain two"),
            "and every other line stays resolved: {rows:?}"
        );

        // Away from it, the emoji goes back to being markup — the reader sees
        // the words and the wash.
        caret_at(&mut d, "two");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == "a red one"),
            "resolved again: {rows:?}"
        );
    }

    #[test]
    fn hidden_modes_never_reveal_wherever_the_caret_is() {
        // The two rungs below `Full` share a rendering: delimiters stay hidden
        // even under the caret. `Shortcuts` differing from `None` only in what
        // typing does is exactly the point of splitting the axes.
        for mode in [MarkupMode::None, MarkupMode::Shortcuts] {
            let mut d = doc_in(View::Wysiwyg, "reveal_hidden", "*one* here\n");
            d.set_markup_mode(mode);
            caret_at(&mut d, "one");
            let rows = drawn_rows(&d);
            assert!(
                rows.iter().any(|r| r == "one here"),
                "{mode:?} hides: {rows:?}"
            );
            assert!(
                !rows.iter().any(|r| r.contains('*')),
                "{mode:?} shows no `*`: {rows:?}"
            );
        }
    }

    #[test]
    fn revealed_delimiters_are_the_authors_own_spelling() {
        // Delimiters are re-read from the source rather than synthesized per
        // kind, so a line comes back spelled the way it was written: `_em_` does
        // not turn into `*em*`, and a two-backtick fence keeps both backticks.
        let body = "_em_ and __st__ and ``lit ` tick`` and [lk](http://x) and ~~del~~\n";
        let mut d = doc_in(View::Wysiwyg, "reveal_spelling", body);
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "em");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == body.trim_end()),
            "the revealed line is its own source: {rows:?}"
        );
    }

    #[test]
    fn revealed_heading_shows_its_hashes() {
        // The `# ` marker is a block-level prefix, not an inline delimiter, so
        // it takes its own path — but it reveals on the same rule.
        let mut d = doc_in(View::Wysiwyg, "reveal_heading", "# Title\n\nbody\n");
        d.set_markup_mode(MarkupMode::Full);

        caret_at(&mut d, "Title");
        assert!(
            drawn_rows(&d).iter().any(|r| r == "# Title"),
            "{:?}",
            drawn_rows(&d)
        );

        caret_at(&mut d, "body");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == "Title"),
            "hashes hidden again: {rows:?}"
        );
    }

    #[test]
    fn revealed_delimiters_are_caret_stops() {
        // A delimiter that is drawn but can't be reached is worse than one
        // that's hidden: the mode exists so the markup can be *edited*. Every
        // revealed byte must be somewhere the caret can stand.
        let mut d = doc_in(View::Wysiwyg, "reveal_stops", "*em* x\n");
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "em");
        let opener = d.source.find('*').unwrap();
        assert!(d.vmap.is_stop(opener), "the opening `*` is a caret stop");
        assert!(
            d.vmap.is_stop(opener + 3),
            "the closing `*` is a caret stop"
        );
    }

    #[test]
    fn setext_heading_reveals_nothing_across_its_newline() {
        // A setext heading's underline is on another line, so it is not the
        // caret line's to reveal — and emitting it would inject a `\n` glyph
        // that splits the row where the author wrote no break.
        let mut d = doc_in(View::Wysiwyg, "reveal_setext", "Title\n=====\n\nbody\n");
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "Title");
        let rows = drawn_rows(&d);
        assert!(
            rows.iter().any(|r| r == "Title"),
            "title renders alone: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains('=')),
            "no underline leaks in: {rows:?}"
        );
    }

    #[test]
    fn markup_mode_axes_split_the_ladder() {
        // The two behaviours the ladder spells: `Shortcuts` is the middle rung
        // that authors markup but still hides it, and it's the only rung where
        // the two axes disagree.
        assert!(!MarkupMode::None.authors());
        assert!(!MarkupMode::None.reveals_caret_line());
        assert!(MarkupMode::Shortcuts.authors());
        assert!(!MarkupMode::Shortcuts.reveals_caret_line());
        assert!(MarkupMode::Full.authors());
        assert!(MarkupMode::Full.reveals_caret_line());
    }

    #[test]
    fn indenting_an_empty_dash_item_under_text_dodges_the_setext_collapse() {
        // Tabbing an empty `- ` under a text line would spell `- hello\n  - `,
        // which twig (correctly, per CommonMark — pandoc agrees) reparses as a
        // setext H2. leaf swaps the dash for a `*` so the item stays an empty
        // nested bullet and `hello` stays prose: the file round-trips instead of
        // hiding a heading the user never asked for.
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "setext_guard", "- hello\n- \n");
            d.caret = d.source.find("- \n").unwrap() + 2; // after the empty marker
            d.indent();
            assert_eq!(d.source, "- hello\n  * \n");
            assert!(
                d.nodes().iter().all(|n| n.kind != Kind::Heading),
                "no heading"
            );
            // And it's genuinely a nested list, not a flat one.
            assert_eq!(
                d.nodes()
                    .iter()
                    .filter(|n| n.kind == Kind::BulletList)
                    .count(),
                2
            );
        }
    }

    #[test]
    fn indenting_a_dash_item_with_content_keeps_its_dash() {
        // With content, `- x` can't be a setext underline, so there's nothing to
        // dodge: the marker stays a dash and nests as an ordinary sub-bullet.
        let mut d = doc_in(View::Wysiwyg, "setext_ok", "- hello\n- x\n");
        d.caret = d.source.find('x').unwrap();
        d.indent();
        assert_eq!(d.source, "- hello\n  - x\n");
    }

    #[test]
    fn the_setext_swap_undoes_as_one_step_with_the_indent() {
        // The dash→`*` repair coalesces into the Tab, so a single undo restores
        // the whole pre-Tab state rather than stranding a half-collapsed doc.
        let mut d = doc_in(View::Wysiwyg, "setext_undo", "- hello\n- \n");
        d.caret = d.source.find("- \n").unwrap() + 2;
        d.indent();
        assert_eq!(d.source, "- hello\n  * \n");
        d.undo();
        assert_eq!(d.source, "- hello\n- \n", "one undo, not two");
    }

    #[test]
    fn indent_leaves_a_nested_lists_first_item_put_too() {
        // The guard is about siblings, not depth: the first item of an *inner*
        // list (already nested under `a`) still has nothing before it at its own
        // level, so Tab can't take it deeper.
        let mut d = doc_in(View::Wysiwyg, "indent_first_nested", "- a\n  - b\n  - c\n");
        d.caret = d.source.find('b').unwrap();
        d.indent();
        assert_eq!(d.source, "- a\n  - b\n  - c\n", "inner first item holds");
        // But `c` (a sibling of `b`) nests under `b`.
        d.caret = d.source.find('c').unwrap();
        d.indent();
        assert_eq!(d.source, "- a\n  - b\n    - c\n");
    }

    #[test]
    fn backspace_at_a_nested_item_start_outdents_it() {
        // Backspace with the caret right after a nested item's marker gives back
        // one level of nesting, the mirror of Tab — and renumbers the flattened
        // ordered list back to a clean run.
        let mut d = doc_in(View::Wysiwyg, "bsp_outdent", "1. a\n   1. b\n2. c\n");
        d.caret = d.source.find('b').unwrap(); // start of the nested item's content
        d.backspace();
        assert_eq!(d.source, "1. a\n2. b\n3. c\n");
    }

    #[test]
    fn backspace_at_a_top_level_item_start_strips_the_marker() {
        // At the outermost level there's no nesting left to give back, so the same
        // keystroke drops the bullet and leaves a plain paragraph.
        let mut d = doc_in(View::Wysiwyg, "bsp_strip", "- a\n- b\n");
        d.caret = d.source.find('b').unwrap(); // right after `- `
        d.backspace();
        assert_eq!(d.source, "- a\nb\n", "the marker is gone, the text stays");
    }

    #[test]
    fn backspace_mid_item_still_deletes_a_character() {
        // The list behaviour is armed only at the item's content start; anywhere
        // else Backspace is the ordinary character delete.
        let mut d = doc_in(View::Wysiwyg, "bsp_mid", "- ab\n");
        d.caret = d.source.find('b').unwrap(); // between `a` and `b`
        d.backspace();
        assert_eq!(d.source, "- b\n");
    }

    #[test]
    fn backspace_at_a_heading_start_strips_the_marker() {
        // The `# ` is markup the rich view hides, so Backspace over it takes the
        // whole marker and leaves a paragraph. Deleting a byte of it instead left
        // `#Title` — no longer a heading, with the hash now literal text the user
        // never typed and has to delete again.
        let mut d = doc_in(View::Wysiwyg, "bsp_head", "## Title\n");
        d.caret = d.source.find('T').unwrap(); // right after `## `
        d.backspace();
        assert_eq!(d.source, "Title\n");
        assert_eq!(
            d.caret, 0,
            "the caret stays with the text it was in front of"
        );
    }

    #[test]
    fn backspace_at_a_heading_start_keeps_the_block_around_it() {
        // Only the heading's own marker goes — the quote (or list) it sits in is
        // untouched, exactly as un-heading it should be.
        let mut d = doc_in(View::Wysiwyg, "bsp_head_quote", "> # Title\n");
        d.caret = d.source.find('T').unwrap();
        d.backspace();
        assert_eq!(d.source, "> Title\n");
    }

    #[test]
    fn backspace_at_a_heading_start_takes_its_closing_sequence_too() {
        // `# Title #`'s trailing hashes are hidden at the other end; leaving them
        // behind would surface the same stray hash the marker delete just avoided.
        let mut d = doc_in(View::Wysiwyg, "bsp_head_closed", "# Title #\n");
        d.caret = d.source.find('T').unwrap();
        d.backspace();
        assert_eq!(d.source, "Title\n");
        // And it's one edit: a single undo puts the whole heading back.
        d.undo();
        assert_eq!(d.source, "# Title #\n");
    }

    #[test]
    fn backspace_mid_heading_still_deletes_a_character() {
        // The heading behaviour is armed only at the content's start; anywhere
        // else Backspace is the ordinary character delete.
        let mut d = doc_in(View::Wysiwyg, "bsp_head_mid", "# ab\n");
        d.caret = d.source.find('b').unwrap();
        d.backspace();
        assert_eq!(d.source, "# b\n");
    }

    #[test]
    fn source_view_backspace_still_edits_the_heading_marker_literally() {
        // In source view the `# ` is text on the screen the user is deleting a
        // byte of, so it keeps its literal meaning — the same split the list
        // ladder and Enter draw between the two views.
        let mut d = doc_with("bsp_head_src", "# Title\n");
        d.caret = d.source.find('T').unwrap();
        d.backspace();
        assert_eq!(d.source, "#Title\n");
    }

    #[test]
    fn outdent_unnests_an_ordered_item_in_one_press() {
        // Shift+Tab gives back exactly the marker width the indent added, so a
        // nested ordered item unnests in a single press, and the flattened list
        // renumbers back to a clean 1, 2, 3.
        let mut d = doc_with("outdent_ord", "1. a\n   2. b\n3. c\n");
        d.caret = d.source.find('b').unwrap();
        d.outdent();
        assert_eq!(d.source, "1. a\n2. b\n3. c\n");
        let lists = d
            .nodes()
            .iter()
            .filter(|n| n.kind == Kind::OrderedList)
            .count();
        assert_eq!(lists, 1, "back to one flat list");
    }

    #[test]
    fn table_insert_row_adds_a_row_below_the_caret() {
        let mut d = doc_with("tbl_ins_row", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        d.caret = d.source.find('1').unwrap(); // in the body row
        d.table_insert_row(true);
        assert_eq!(d.source, "| a | b |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n");
    }

    #[test]
    fn table_insert_and_delete_column_at_the_caret() {
        let mut d = doc_with("tbl_col", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        d.caret = d.source.find('a').unwrap(); // column 0
        d.table_insert_column(true); // add a column to the right of `a`
        assert_eq!(
            d.source,
            "| a |  | b |\n| --- | --- | --- |\n| 1 |  | 2 |\n"
        );
        d.caret = d.source.find('b').unwrap(); // now the third column
        d.table_delete_column();
        assert_eq!(d.source, "| a |  |\n| --- | --- |\n| 1 |  |\n");
    }

    // ── ragged formats ───────────────────────────────────────────────────────
    // No format spells every gesture. HTML writes the inline marks as a tag pair
    // and no heading, list, quote or link; Markdown spells five of the eight
    // marks — the highlight only because leaf parses with `highlight`, which is
    // why the question is asked with the extensions; djot spells all eight and
    // no in-cell break. leaf asks twig per
    // gesture (`Doc::supports`) and refuses at the door, rather than letting each
    // op discover the fact on its own — one of them didn't.

    /// An HTML document in the rich view, ready for a gesture.
    fn html_doc(body: &str) -> Doc {
        let mut d = Doc::from_source(body.to_string(), Format::Html).unwrap();
        d.view = View::Wysiwyg;
        d.build_visual(80);
        d
    }

    #[test]
    fn a_table_gesture_leaves_an_html_table_alone() {
        // The regression this guard exists for. twig's table editor consults no
        // `Syntax` table — it spells a grid, not a delimiter — so it rebuilt an
        // HTML `<table>` as a *pipe table* and reported success: the whole
        // element replaced by `| a | b |`, silently, on one press of a toolbar
        // button. Every grid op went the same way.
        let src = "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>\n";
        // A table of named operations, which is what it looks like.
        #[allow(clippy::type_complexity)]
        let ops: [(&str, &dyn Fn(&mut Doc)); 7] = [
            ("insert row", &|d: &mut Doc| d.table_insert_row(true)),
            ("delete row", &|d: &mut Doc| d.table_delete_row()),
            ("insert column", &|d: &mut Doc| d.table_insert_column(true)),
            ("delete column", &|d: &mut Doc| d.table_delete_column()),
            ("align", &|d: &mut Doc| {
                d.table_set_alignment(Alignment::Right)
            }),
            ("move row", &|d: &mut Doc| d.table_move_row(true)),
            ("move column", &|d: &mut Doc| d.table_move_column(true)),
        ];
        for (name, op) in ops {
            let mut d = html_doc(src);
            d.caret = d.source.find('a').unwrap();
            assert!(d.caret_in_table(), "{name}: the caret really is in a table");
            op(&mut d);
            assert_eq!(d.source, src, "{name} rewrote an HTML table");
            assert!(
                !d.dirty,
                "{name} marked the document dirty without editing it"
            );
            assert!(d.status.is_some(), "{name} refused without saying why");
        }
    }

    #[test]
    fn the_block_gestures_html_cannot_spell_are_refused_with_a_reason() {
        // A task box is a form control in HTML and a footnote has no native
        // spelling at all — the two gestures twig 3.5 still spells nothing
        // for, now that a quote, a list, a link and an image print through
        // its renderer (see the test below).
        let src = "<h1>Title</h1>\n<p>Hello world</p>\n<ul><li>one</li></ul>\n";
        // A table of named operations, which is what it looks like.
        #[allow(clippy::type_complexity)]
        let ops: [(&str, &dyn Fn(&mut Doc)); 3] = [
            ("task item", &|d: &mut Doc| d.toggle_task_item()),
            ("task tick", &|d: &mut Doc| d.toggle_task_checked()),
            ("footnote", &|d: &mut Doc| d.insert_footnote()),
        ];
        for (name, op) in ops {
            let mut d = html_doc(src);
            let at = d.source.find("Hello").unwrap();
            d.caret = at;
            d.anchor = Some(at + 5); // a selection, for the ops that want one
            op(&mut d);
            assert_eq!(d.source, src, "{name} edited an HTML document");
            assert!(
                !d.dirty,
                "{name} marked the document dirty without editing it"
            );
            let status = d.status.as_deref().unwrap_or("");
            assert!(
                status.contains("html"),
                "{name}: the refusal should name the format, got {status:?}"
            );
        }
    }

    #[test]
    fn html_spells_a_quote_a_list_a_link_and_an_image_through_the_renderer() {
        // twig 3.5: where HTML has no marker alphabet it prints the fresh
        // node — a `<blockquote>` around the paragraph, a `<ul>`/`<ol>` with
        // the paragraph as its item, an `<a>` or `<img>` over the selection.
        // Until then every one of these was a refusal; now each is a real
        // edit, which is what the toolbar's capability flags say too.
        let src = "<h1>Title</h1>\n<p>Hello world</p>\n<ul><li>one</li></ul>\n";
        #[allow(clippy::type_complexity)]
        let ops: [(&str, &dyn Fn(&mut Doc), &str); 5] = [
            (
                "quote",
                &|d: &mut Doc| d.toggle_blockquote(),
                "<blockquote>",
            ),
            ("list", &|d: &mut Doc| d.toggle_list(false), "<ul>\n<li>"),
            (
                "ordered list",
                &|d: &mut Doc| d.toggle_list(true),
                "<ol>\n<li>",
            ),
            (
                "link",
                &|d: &mut Doc| d.insert_link("https://example.dev"),
                "<a href=\"https://example.dev\">Hello</a>",
            ),
            (
                "image",
                &|d: &mut Doc| d.insert_image("pic.png", "alt"),
                "<img alt=\"Hello\" src=\"pic.png\">",
            ),
        ];
        for (name, op, expect) in ops {
            let mut d = html_doc(src);
            let at = d.source.find("Hello").unwrap();
            d.caret = at;
            d.anchor = Some(at + 5);
            op(&mut d);
            assert!(d.source.contains(expect), "{name}: got {:?}", d.source);
            assert!(d.dirty, "{name}: a real edit");
            assert_eq!(
                d.status, None,
                "{name}: a supported gesture reports nothing"
            );
        }
    }

    #[test]
    fn html_spells_a_heading_as_its_tag_pair() {
        // twig 3.4 rebuilds a heading or paragraph as its tag pair, attributes
        // along — the one block gesture whose HTML shape it can write. So ⌘2
        // in an HTML document is a real edit, and ⌘0 takes it back.
        let src = "<h1>Title</h1>\n<p>Hello world</p>\n";
        let mut d = html_doc(src);
        d.caret = d.source.find("Hello").unwrap();
        d.toggle_heading(2);
        assert_eq!(d.source, "<h1>Title</h1>\n<h2>Hello world</h2>\n");
        assert!(d.dirty);
        assert_eq!(d.status, None, "a supported gesture reports nothing");
        d.toggle_heading(2);
        assert_eq!(d.source, src, "the same level again is back to a paragraph");
    }

    #[test]
    fn html_spells_the_inline_marks_and_the_rule() {
        // The other half, and why one per-document flag stopped being enough:
        // ⌘B in an HTML document writes `<strong>` — the tag the serializer
        // already emits and the parser reads straight back as the same mark —
        // and the rule button writes an `<hr>`. Refusing these on the old
        // "HTML is parse-only" reading would now be leaf's own limitation.
        let mut d = html_doc("<p>Hello world</p>\n");
        let at = d.source.find("world").unwrap();
        d.caret = at;
        d.anchor = Some(at + 5);
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "<p>Hello <strong>world</strong></p>\n");
        assert!(d.dirty);
        assert_eq!(d.status, None, "a supported gesture reports nothing");

        // And off again — the toggle reverses, which is the property that makes
        // authoring in HTML worth offering rather than a one-way trip.
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "<p>Hello world</p>\n");

        let mut d = html_doc("<p>Hello world</p>\n");
        d.caret = d.source.find("world").unwrap();
        d.insert_thematic_break();
        assert!(d.source.contains("<hr>"), "got {:?}", d.source);
    }

    #[test]
    fn a_mark_the_format_cannot_spell_arms_nothing() {
        // `toggle` with a collapsed caret doesn't reach twig at all — it arms a
        // sticky mark for the next text typed. Guarding only the twig call
        // leaves that path live, promising a mark the gesture will not write and
        // then swallowing the error inside `insert`.
        //
        // Markdown carries this, on the superscript now rather than on the
        // highlight: `^x^` is text there in any configuration, whereas twig
        // 3.3.1 authors `==x==` for an editor holding the `highlight` extension,
        // which every leaf document does.
        let mut d = doc_with("mark", "Hello world\n");
        d.view = View::Wysiwyg;
        d.build_visual(80);
        d.caret = d.source.find("world").unwrap();
        d.toggle(InlineKind::Superscript);
        assert!(d.pending_marks.is_empty(), "no mark should be armed");
        assert!(d.status.as_deref().unwrap_or("").contains("markdown"));
        d.insert("X");
        assert_eq!(d.source, "Hello Xworld\n");
    }

    #[test]
    fn markdown_authors_a_highlight_and_a_strikethrough() {
        // twig 3.3.1: the two marks Markdown reads and, until it, refused to
        // write. `==x==` is authorable because leaf's own `parse_extensions`
        // turns `highlight` on — twig will only mint bytes this editor's reparse
        // reads back — and `~~x~~` because GFM strikethrough is parsed by
        // default, so the refusal there was never right for any leaf document.
        // twig 3.10 adds the underline, spelled `<u>x</u>` and paired back into
        // an `insert` under the `html_elements` leaf parses with.
        for (kind, marked) in [
            (InlineKind::Mark, "a ==word== b\n"),
            (InlineKind::Delete, "a ~~word~~ b\n"),
            (InlineKind::Insert, "a <u>word</u> b\n"),
        ] {
            let mut d = doc_with("author_mark", "a word b\n");
            d.anchor = Some(2);
            d.caret = 6;
            d.toggle(kind);
            assert_eq!(d.source, marked, "{kind:?}");
            assert_eq!(d.status, None, "{kind:?}: a supported gesture is silent");
            assert!(d.dirty, "{kind:?}");
            // The region stays selected, so the second press reverses it — the
            // property that separates authoring from a one-way trip.
            d.toggle(kind);
            assert_eq!(d.source, "a word b\n", "{kind:?}");
        }
    }

    #[test]
    fn an_authored_highlight_reads_back_as_a_mark() {
        // The round trip the extension gate exists to protect: what the toggle
        // writes, the reparse must read back as a `mark` rather than as two
        // literal `=` pairs. A `Role::Mark` glyph is that answer, taken from the
        // rebuilt map rather than from the source text.
        let mut d = doc_with("mark_roundtrip", "a word b\n");
        d.view = View::Wysiwyg;
        d.build_visual(80);
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Mark);
        assert_eq!(d.source, "a ==word== b\n");
        d.build_visual(80);
        let w = d
            .vmap
            .rows
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .find(|g| g.ch == 'w')
            .expect("the highlighted word");
        assert_eq!(w.style.role, crate::Role::Mark(None));
    }

    #[test]
    fn an_authored_underline_reads_back_as_an_insert() {
        // The same round trip for `<u>…</u>`: under `html_elements` the tag pair
        // is one `insert`, so the word is underlined and the tags are markup the
        // caret-off line hides — not two `raw_inline` runs drawn as literal
        // HTML around plain text.
        let mut d = doc_with("underline_roundtrip", "a word b\nnext\n");
        d.view = View::Wysiwyg;
        d.build_visual(80);
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Insert);
        assert_eq!(d.source, "a <u>word</u> b\nnext\n");
        d.anchor = None;
        d.caret = d.source.find("next").unwrap();
        d.build_visual(80);
        let glyphs: Vec<_> = d.vmap.rows.iter().flat_map(|r| r.glyphs.iter()).collect();
        let w = glyphs
            .iter()
            .find(|g| g.ch == 'w')
            .expect("the underlined word");
        assert!(w.style.underline);
        assert!(!glyphs.iter().any(|g| g.ch == '<'), "the tags are hidden");
    }

    #[test]
    fn a_highlight_takes_a_colour_changes_it_and_gives_it_back() {
        // The three states of one gesture, in the order a palette is pressed:
        // an uncoloured highlight takes the prefix, a coloured one has it
        // replaced, and `None` takes it away with the space that was part of the
        // spelling.
        let mut d = doc_with("mark_colour", "a ==word== b\n");
        d.caret = d.source.find("word").unwrap();
        d.set_mark_color(Some(MarkColor::Red));
        assert_eq!(d.source, "a ==🔴 word== b\n");
        assert_eq!(d.status, None);
        assert!(d.dirty);

        d.set_mark_color(Some(MarkColor::Blue));
        assert_eq!(d.source, "a ==🔵 word== b\n");

        d.set_mark_color(None);
        assert_eq!(d.source, "a ==word== b\n");
    }

    #[test]
    fn the_caret_keeps_its_place_in_the_text_across_a_colour() {
        // The prefix is written *before* the word, so an offset in the word has
        // to ride its width — a caret that stayed put would be a caret that
        // walked backwards through the text it was standing in.
        let mut d = doc_with("mark_colour_caret", "a ==word== b\n");
        let word = d.source.find("word").unwrap();
        d.caret = word + 2; // between `wo` and `rd`
        d.set_mark_color(Some(MarkColor::Red));
        assert_eq!(&d.source[d.caret..d.caret + 2], "rd", "still before `rd`");

        // And back the other way when the prefix goes.
        d.set_mark_color(None);
        assert_eq!(&d.source[d.caret..d.caret + 2], "rd");
    }

    #[test]
    fn the_colour_at_the_caret_is_what_the_palette_lights() {
        let mut d = doc_with("mark_colour_read", "a ==🔴 red== and ==plain== b\n");
        d.caret = d.source.find("red").unwrap();
        assert!(d.caret_in_mark());
        assert_eq!(d.mark_color_at_caret(), Some(MarkColor::Red));

        d.caret = d.source.find("plain").unwrap();
        assert!(d.caret_in_mark(), "a highlight with no colour is still one");
        assert_eq!(d.mark_color_at_caret(), None);

        d.caret = d.source.find(" and ").unwrap() + 2;
        assert!(!d.caret_in_mark());
        assert_eq!(d.mark_color_at_caret(), None);
    }

    #[test]
    fn a_colour_without_a_highlight_says_so_and_writes_nothing() {
        // The gesture colours a highlight that exists; it does not make one.
        // Two presses is the price of a coloured highlight from bare text, and
        // the reason is undo — one press that spliced twice would take two
        // presses to take back.
        let mut d = doc_with("mark_colour_none", "a word b\n");
        d.caret = d.source.find("word").unwrap();
        d.set_mark_color(Some(MarkColor::Red));
        assert_eq!(d.source, "a word b\n");
        assert!(d.status.is_some(), "it should say why");
        assert!(!d.dirty);

        // Clearing where there is nothing to clear is the same refusal, not a
        // quiet success — the caret is in no highlight either way.
        d.status = None;
        d.set_mark_color(None);
        assert_eq!(d.source, "a word b\n");
        assert!(d.status.is_some());
    }

    #[test]
    fn clearing_an_uncoloured_highlight_is_a_quiet_no_op() {
        // twig answers this one *successfully* with a `Change` describing some
        // earlier edit, so a caller that trusted the change would jump the caret
        // to wherever that was. Core answers it before asking.
        let mut d = doc_with("mark_colour_noop", "a ==word== b\n");
        d.toggle(InlineKind::Strong); // an earlier edit for a stale change to name
        d.caret = d.source.find("word").unwrap();
        let (source, caret) = (d.source.clone(), d.caret);
        d.set_mark_color(None);
        assert_eq!(d.source, source);
        assert_eq!(
            d.caret, caret,
            "the caret must not ride a change that isn't one"
        );
        assert_eq!(d.status, None, "and it is not an error either");
    }

    #[test]
    fn djot_spells_the_highlight_and_not_its_colour() {
        // The reason the palette is its own capability rather than the Highlight
        // button's: `{=word=}` is a highlight djot writes happily, and there is
        // no djot spelling for a colour on it.
        assert!(Capabilities::of(Format::Djot).mark);
        assert!(!Capabilities::of(Format::Djot).mark_color);
        assert!(Capabilities::of(Format::Markdown).mark_color);

        let mut d = Doc::from_source("a {=word=} b\n".into(), Format::Djot).unwrap();
        d.caret = d.source.find("word").unwrap();
        assert!(
            d.caret_in_mark(),
            "the caret is in a highlight all the same"
        );
        d.set_mark_color(Some(MarkColor::Red));
        assert_eq!(d.source, "a {=word=} b\n");
        assert!(
            d.status.as_deref().unwrap_or("").contains("djot"),
            "and the refusal names the document's format: {:?}",
            d.status
        );
    }

    #[test]
    fn a_coloured_highlight_is_one_undo_step_and_reads_back_as_its_colour() {
        // The round trip that matters for a palette: the bytes twig writes are
        // bytes its own reparse reads back as a colour, so the swatch that was
        // pressed is the swatch that lights afterwards.
        let mut d = doc_with("mark_colour_undo", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Mark);
        d.caret = d.source.find("word").unwrap();
        d.set_mark_color(Some(MarkColor::Green));
        assert_eq!(d.source, "a ==🟢 word== b\n");
        assert_eq!(d.mark_color_at_caret(), Some(MarkColor::Green));

        // One splice, one step: the colour comes off and the highlight stays.
        d.undo();
        assert_eq!(d.source, "a ==word== b\n");
        d.undo();
        assert_eq!(d.source, "a word b\n");
    }

    #[test]
    fn every_colour_leaf_names_is_one_twig_writes() {
        // The two enums are one vocabulary, and this is what says so: each of
        // leaf's colours writes an emoji twig's reparse reads back as *that*
        // colour, so `twig_mark_color`'s table cannot quietly pair red with
        // orange.
        for color in MarkColor::ALL {
            let mut d = doc_with("mark_colour_all", "a ==word== b\n");
            d.caret = d.source.find("word").unwrap();
            d.set_mark_color(Some(color));
            assert_eq!(d.status, None, "{color:?}");
            assert_eq!(d.mark_color_at_caret(), Some(color), "{color:?}");
        }
    }

    #[test]
    fn a_fresh_highlight_takes_a_colour_without_moving_the_caret_first() {
        // The two presses a coloured highlight is made of, in the state the
        // first one leaves: `toggle` selects the whole `==word==` and puts the
        // caret one past the closing `==`, which is *not* in the mark. Asking at
        // the caret alone would refuse to colour the highlight just written —
        // the selection's start is what answers.
        let mut d = doc_with("mark_colour_fresh", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.toggle(InlineKind::Mark);
        assert_eq!(d.source, "a ==word== b\n");
        assert_eq!(d.caret, 10, "the caret twig leaves, past the closing `==`");

        assert!(d.caret_in_mark(), "the selected highlight is the one meant");
        d.set_mark_color(Some(MarkColor::Yellow));
        assert_eq!(d.source, "a ==🟡 word== b\n");
        assert_eq!(d.status, None);
    }

    #[test]
    fn one_press_highlights_a_selection_and_colours_it() {
        // What a toolbar swatch means over a plain selection, and the undo it
        // has to have: one press, one step. Two steps would leave an uncoloured
        // highlight behind on the way back, which is a state the author never
        // asked for and never saw.
        let mut d = doc_with("highlight_one", "a word b\n");
        d.anchor = Some(2);
        d.caret = 6;
        d.highlight(Some(MarkColor::Purple));
        assert_eq!(d.source, "a ==\u{1F7E3} word== b\n");
        assert_eq!(d.status, None);

        d.undo();
        assert_eq!(d.source, "a word b\n", "one press, one undo");
    }

    #[test]
    fn one_press_on_an_existing_highlight_only_recolours_it() {
        // The other half: inside a highlight there is nothing to make, so the
        // compound is the plain gesture and the text is untouched.
        let mut d = doc_with("highlight_recolour", "a ==\u{1F534} word== b\n");
        d.caret = d.source.find("word").unwrap();
        d.highlight(Some(MarkColor::Blue));
        assert_eq!(d.source, "a ==\u{1F535} word== b\n");
        d.undo();
        assert_eq!(d.source, "a ==\u{1F534} word== b\n", "the highlight stays");
    }

    #[test]
    fn one_press_with_no_colour_over_a_selection_just_highlights_it() {
        // `None` means "no colour", and over bare text that is the Highlight
        // button's own job. The fold must not happen here — there is no second
        // splice, and folding would take the *previous* edit into this one.
        let mut d = doc_with("highlight_none", "a word b and more\n");
        d.caret = d.source.find("more").unwrap() + 4; // after "more"
        d.insert("!"); // an earlier edit for a wrong fold to swallow
        d.anchor = Some(2);
        d.caret = 6;
        d.highlight(None);
        assert_eq!(d.source, "a ==word== b and more!\n");

        d.undo();
        assert_eq!(
            d.source, "a word b and more!\n",
            "only the highlight came off"
        );
        d.undo();
        assert_eq!(
            d.source, "a word b and more\n",
            "and the edit before it survived"
        );
    }

    #[test]
    fn one_press_at_a_bare_caret_in_no_highlight_writes_nothing() {
        // `toggle` at a collapsed caret arms a mark for text not yet typed, and
        // a colour cannot be armed with it — so the compound declines rather
        // than leaving half a promise.
        let mut d = doc_with("highlight_bare", "a word b\n");
        d.caret = 4;
        d.highlight(Some(MarkColor::Red));
        assert_eq!(d.source, "a word b\n");
        assert!(d.pending_marks.is_empty(), "and nothing armed");
        assert!(d.status.is_some());
    }

    #[test]
    fn a_read_only_document_takes_no_colour() {
        let mut d = doc_with("mark_colour_ro", "a ==word== b\n");
        d.caret = d.source.find("word").unwrap();
        d.set_read_only(true);
        d.set_mark_color(Some(MarkColor::Red));
        assert_eq!(d.source, "a ==word== b\n");
    }

    #[test]
    fn a_sticky_highlight_wraps_the_next_typed_text_in_markdown() {
        // The other door into `toggle`: no selection, so nothing reaches twig
        // until `insert` realises the armed mark. It is armed now — the guard
        // above asks `Doc::supports`, which asks with the extensions — and what
        // it writes is the same `==…==`.
        let mut d = doc_with("sticky_mark", "xy\n");
        d.caret = 1;
        d.toggle(InlineKind::Mark);
        assert!(d.pending_marks.contains(InlineKind::Mark));
        d.insert("Z");
        assert_eq!(d.source, "x==Z==y\n");
    }

    #[test]
    fn html_documents_still_take_typed_text() {
        // The guard covers *markup* gestures and must not touch plain editing:
        // twig's splicer is language-neutral, and typing into an HTML document
        // is the thing that does work today.
        let mut d = html_doc("<p>Hello world</p>\n");
        d.caret = d.source.find("world").unwrap();
        d.insert("big ");
        assert_eq!(d.source, "<p>Hello big world</p>\n");
        assert!(d.dirty);
        d.backspace();
        assert_eq!(d.source, "<p>Hello bigworld</p>\n");
        d.undo();
        d.undo();
        assert_eq!(d.source, "<p>Hello world</p>\n");
    }

    #[test]
    fn authorable_is_the_coarse_question_and_capabilities_the_useful_one() {
        // `authorable` only separates "there is a door in" from "there is not",
        // and HTML is on the near side of that line — which is exactly why a
        // toolbar must not be built from it.
        let html = Doc::from_source("<p>x</p>\n".into(), Format::Html).unwrap();
        assert!(html.authorable());
        assert!(
            !Doc::from_source("<r>x</r>".into(), Format::Xml)
                .unwrap()
                .authorable()
        );

        let caps = html.capabilities();
        assert!(caps.bold && caps.italic && caps.code && caps.mark);
        assert!(caps.thematic_break && caps.cell_line_break);
        // A heading is a tag pair twig rebuilds (3.4), and since 3.5 so are a
        // quote, a list, a code block's language, a link and an image — each
        // printed as a fresh node where HTML has no marker to rewrite. A task
        // box is a form control and a footnote has no spelling, so those two
        // are what keeps the record ragged.
        assert!(caps.heading && caps.blockquote && caps.bullet_list);
        assert!(caps.link && caps.image && caps.code_language);
        assert!(!caps.task && !caps.footnote);
        // The one flag that isn't twig's answer: an HTML `<table>` is a grid
        // twig's table editor would happily re-emit as `| a | b |`.
        assert!(!caps.table);

        // The two lightweight formats spell everything leaf offers — and still
        // differ from each other, which is the other half of why one boolean
        // can't serve.
        for fmt in [Format::Markdown, Format::Djot] {
            let caps = Capabilities::of(fmt);
            assert!(
                caps.heading && caps.blockquote && caps.ordered_list,
                "{fmt:?}"
            );
            assert!(
                caps.task && caps.link && caps.image && caps.table,
                "{fmt:?}"
            );
        }
        // Both spell the highlight, the strikethrough and the underline: djot
        // natively, and Markdown because `Capabilities` asks with
        // `parse_extensions` rather than with twig's defaults — `==x==` is text
        // under those, and a mark under the `highlight` leaf always parses
        // with; `<u>x</u>` likewise pairs into an `insert` only under
        // `html_elements`.
        for fmt in [Format::Markdown, Format::Djot] {
            let caps = Capabilities::of(fmt);
            assert!(caps.mark && caps.strike && caps.underline, "{fmt:?}");
        }
        // What still separates them, now that the highlight doesn't: djot has
        // no in-cell break, and Markdown spells neither of the scripts.
        assert!(Capabilities::of(Format::Djot).superscript);
        assert!(!Capabilities::of(Format::Markdown).superscript);
        assert!(Capabilities::of(Format::Markdown).cell_line_break);
        assert!(!Capabilities::of(Format::Djot).cell_line_break);

        // A parse-only format answers no to every one of them, so the coarse
        // predicate and the record agree there.
        let caps = Capabilities::of(Format::Xml);
        assert!(!caps.bold && !caps.heading && !caps.table && !caps.thematic_break);
    }

    #[test]
    fn a_refused_gesture_says_so_where_twig_would_have_said_it() {
        // The guard exists to name the *document's* format rather than twig's
        // internals, so the message has to survive being one leaf writes itself.
        // Checked against a gesture twig also refuses, since that is the pair
        // most at risk of drifting apart — the task box, once the code
        // language stopped being one (twig 3.5).
        let mut d = html_doc("<p>Hello</p>\n");
        d.caret = d.source.find("Hello").unwrap();
        d.toggle_task_item();
        assert_eq!(d.status.as_deref(), Some("task: not supported in html"));
        assert!(!d.dirty);
    }

    #[test]
    fn table_set_alignment_respells_the_delimiter() {
        let mut d = doc_with("tbl_align", "| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        d.caret = d.source.find('b').unwrap();
        d.table_set_alignment(Alignment::Right);
        assert_eq!(d.source, "| a | b |\n| --- | ---: |\n| 1 | 2 |\n");
    }

    #[test]
    fn each_empty_table_cell_has_its_own_editable_home() {
        // Regression: an empty cell has no twig content_span, so both cells of a
        // `|  |  |` row collapsed onto the row's start (before the first `│`).
        // Typing there inserted *before* the table (`hello|  |  |`); nav couldn't
        // tell the cells apart. Each empty cell must now have a distinct home
        // inside it.
        let mut d = wysiwyg_doc("tbl_empty", "| a | b |\n| --- | --- |\n|  |  |\n");
        let (c0, c1) = {
            let cells = &d.vmap.tables[0].grid[1].cells;
            (cells[0].start, cells[1].start)
        };
        assert!(
            c0 < c1,
            "the two empty cells have distinct homes: {c0} < {c1}"
        );
        d.caret = c0;
        d.insert("x");
        assert_eq!(
            d.source, "| a | b |\n| --- | --- |\n| x |  |\n",
            "typed inside the cell"
        );
    }

    #[test]
    fn arrows_step_into_each_empty_table_cell() {
        let mut d = wysiwyg_doc("tbl_empty_nav", "| a | b |\n| --- | --- |\n|  |  |\n");
        let (c0, c1) = {
            let cells = &d.vmap.tables[0].grid[1].cells;
            (cells[0].start, cells[1].start)
        };
        d.caret = d.source.find('b').unwrap(); // in the header's second cell
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6 {
            d.move_right(false);
            seen.insert(d.caret);
        }
        assert!(
            seen.contains(&c0),
            "right arrow reaches the first empty cell"
        );
        assert!(
            seen.contains(&c1),
            "right arrow reaches the second empty cell"
        );
    }

    #[test]
    fn table_op_off_a_table_is_a_no_op_with_a_status() {
        let mut d = doc_with("tbl_none", "just text\n");
        d.caret = 3;
        d.table_insert_row(true);
        assert_eq!(d.source, "just text\n", "nothing changed");
        assert!(d.status.is_some(), "a status explains why");
        assert!(!d.caret_in_table());
    }

    #[test]
    fn enter_in_an_ordered_list_renumbers_the_following_items() {
        // Inserting an item mid-list left the source markers stale (`1. 2. 2. 3.`);
        // the renumber pass keeps them sequential, matching what the view draws.
        let mut d = wysiwyg_doc("enter_renumber", "1. a\n2. b\n3. c\n");
        d.caret = d.source.find('a').unwrap() + 1; // end of item a
        d.newline();
        d.insert("x");
        assert_eq!(d.source, "1. a\n2. x\n3. b\n4. c\n");
    }

    #[test]
    fn outdent_with_nothing_to_give_back_records_no_undo_step() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "outdent_noop", "hello\n");
            d.caret = 2;
            d.outdent();
            assert_eq!(d.source, "hello\n");
            assert!(!d.dirty, "a no-op is not a modification");
            d.undo();
            assert_eq!(
                d.status.as_deref(),
                Some("nothing to undo"),
                "spends no undo step"
            );
            assert_eq!(d.source, "hello\n");
        }
    }

    #[test]
    fn indent_shifts_every_selected_line_and_keeps_them_selected() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_sel", "one\n\ntwo\n");
            d.anchor = Some(0);
            d.caret = 7; // through "two"
            d.indent();
            assert_eq!(
                d.source, "  one\n\n  two\n",
                "the blank line keeps no trailing pad"
            );
            // Selected, so a second Tab lands on the same lines rather than on
            // whatever the shifted offsets now cover.
            assert_eq!(d.selection(), Some((0, 12)));
            d.indent();
            assert_eq!(d.source, "    one\n\n    two\n");
        }
    }

    #[test]
    fn outdent_takes_what_each_line_has_and_leaves_the_rest_alone() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "outdent_sel", "  two\n one\nnone\n");
            d.anchor = Some(0);
            d.caret = 15;
            d.outdent();
            assert_eq!(d.source, "two\none\nnone\n");
        }
    }

    #[test]
    fn a_tab_undoes_as_one_step_however_many_lines_it_moved() {
        for view in [View::Source, View::Wysiwyg] {
            let mut d = doc_in(view, "indent_undo", "one\n\ntwo\n");
            d.anchor = Some(0);
            d.caret = 7;
            d.indent();
            assert_eq!(d.source, "  one\n\n  two\n");
            d.undo();
            assert_eq!(d.source, "one\n\ntwo\n", "one step, not one per line");
            assert_eq!(
                d.selection(),
                Some((0, 7)),
                "with the selection it was aimed at"
            );
            d.redo();
            assert_eq!(d.source, "  one\n\n  two\n");
            assert_eq!(
                d.selection(),
                Some((0, 12)),
                "redo replays the caret the indent placed, not the one splice left"
            );
        }
    }

    #[test]
    fn vertical_motion_keeps_the_column() {
        let mut d = doc_with("move", "abcd\nef\n");
        d.caret = 3; // "abc|d" on row 0, col 3
        d.move_down(false); // row 1 "ef" only has cols 0..2 -> clamps to end
        assert_eq!(d.caret, 7); // just after "ef"
    }

    // ── goal column ──────────────────────────────────────────────────────────

    #[test]
    fn vertical_motion_goal_column_survives_a_short_line() {
        // Regression: re-deriving the column from the clamped position on
        // every step permanently forgets it once a short line clamps it.
        // Down through "xy" (2 cols) and into "ghijkl" must return to col 4.
        let g = |m, f: fn(&mut Doc)| golden("goalcol", m, f);
        assert_eq!(
            g("abcd|ef\nxy\nghijkl\n", |d| {
                d.move_down(false); // clamps to end of "xy"
                d.move_down(false); // restores col 4 on the long line
            }),
            "abcdef\nxy\nghij|kl\n"
        );
    }

    #[test]
    fn goal_column_state_is_set_by_vertical_motion_and_cleared_by_horizontal() {
        let mut d = doc_with("goalcol_state", "abcdef\nxy\nghijkl\n");
        assert_eq!(d.goal_col, None);
        d.caret = 4; // row 0, col 4
        d.move_down(false); // clamps into "xy"; goal stays the original col
        assert_eq!(d.goal_col, Some(4));
        assert_eq!(d.caret_pos(), (1, 2));

        // A horizontal motion drops the goal column...
        d.move_left(false);
        assert_eq!(d.goal_col, None);

        // ...so the next vertical motion picks up the *new* column (1), not
        // the stale one (4).
        d.move_down(false);
        assert_eq!(d.goal_col, Some(1));
        assert_eq!(d.caret_pos(), (2, 1));
    }

    #[test]
    fn editing_clears_the_goal_column() {
        let mut d = doc_with("goalcol_edit", "abcdef\nxy\nghijkl\n");
        d.caret = 4;
        d.move_down(false);
        assert_eq!(d.goal_col, Some(4));
        d.insert("Z");
        assert_eq!(d.goal_col, None);
    }

    #[test]
    fn vertical_motion_on_an_empty_document_is_a_no_op() {
        let mut d = doc_with("empty_vert", "");
        d.move_down(false);
        assert_eq!(d.caret, 0);
        d.move_up(false);
        assert_eq!(d.caret, 0);
    }

    // ── the document's edges ─────────────────────────────────────────────────

    #[test]
    fn vertical_motion_at_the_document_edges_runs_to_them_in_both_views() {
        // The reproduction, and the disagreement: Down on the last line ran to
        // the end of the document in the source view — by accident, an
        // out-of-range row clamping to the end of the string — and did nothing
        // whatever in the view leaf opens in. One rule now, in both.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("edge_{tag}"), "abc");
            d.caret = 1;
            d.move_down(false);
            assert_eq!(d.caret, 3, "{tag}: Down on the last line runs to the end");
            d.move_up(false);
            assert_eq!(d.caret, 0, "{tag}: Up on the first line runs to the start");
        }
    }

    #[test]
    fn vertical_motion_at_the_edges_carries_the_column_across_the_lines_between() {
        // Down off the bottom is a motion like any other, so it latches a goal
        // column — and Up comes back to the column the caret left, not to the
        // one the document's end happened to be in.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("abcdef{gap}ghijkl");
            let mut d = doc_in(view, &format!("edge_goal_{tag}"), &src);
            d.caret = 2; // row 0, col 2
            d.move_down(false);
            assert_eq!(d.caret_pos().1, 2, "{tag}: Down keeps the column");
            d.move_down(false);
            assert_eq!(
                d.caret,
                src.len(),
                "{tag}: Down off the bottom reaches the end"
            );
            d.move_up(false);
            assert_eq!(
                d.caret_pos().1,
                2,
                "{tag}: Up returns to the column Down left"
            );
        }
    }

    #[test]
    fn vertical_motion_with_nowhere_to_go_latches_no_goal_column() {
        // `goal_col.get_or_insert` ran *before* the early return at row 0, so an
        // Up that did nothing still armed a goal column, and the next Down aimed
        // at a column the caret had never been in.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("noop_goal_{tag}"), "abc\n\ndef");
            d.caret = 0;
            d.move_up(false);
            assert_eq!(d.caret, 0, "{tag}: already at the start");
            assert_eq!(d.goal_col, None, "{tag}: a no-op Up latched a goal column");

            d.caret = d.source.len();
            d.move_down(false);
            assert_eq!(d.caret, d.source.len(), "{tag}: already at the end");
            assert_eq!(
                d.goal_col, None,
                "{tag}: a no-op Down latched a goal column"
            );
        }
    }

    // ── soft wrap ────────────────────────────────────────────────────────────
    // Every other test here builds the map at 80 columns, where no fixture is
    // long enough to fold. A wrap is where one offset belongs to two rows at
    // once, and it broke everything that asks the caret what row it is on.

    /// The wrapped fixture these cases share, folded at 12 columns into
    /// `one two ` / `three four ` / `five six ` / `seven eight`.
    fn wrapped_doc(name: &str) -> Doc {
        let mut d = wysiwyg_doc(name, "one two three four five six seven eight");
        d.build_visual(12);
        d
    }

    #[test]
    fn home_and_end_work_from_a_wrapped_row() {
        // The reproduction: offset 19 is the `f` of "five", the first character
        // of the third row — and also the offset the second row ends at. It
        // resolved to the *second* row, so End aimed at a place the caret was
        // already in and did nothing, while Home walked backwards onto a row the
        // caret had left.
        let mut d = wrapped_doc("wrap_home_end");
        d.caret = 19;
        assert_eq!(
            d.caret_pos(),
            (2, 0),
            "the wrap boundary opens the third row"
        );
        d.move_end(false);
        assert_eq!(d.caret, 27, "End stalled at the wrap boundary");
        d.move_home(false);
        assert_eq!(d.caret, 19, "Home left the row the caret was on");
    }

    #[test]
    fn end_of_a_wrapped_row_stays_put_when_pressed_again() {
        // The row's end is the last offset that is only ever its own: the offset
        // past it opens the row below, and aiming there would send a second
        // press on to *that* row's end, and a third to the next — End walking
        // down the paragraph rather than sitting where it landed.
        let mut d = wrapped_doc("wrap_end_twice");
        d.caret = 12; // inside "three", on the second row
        d.move_end(false);
        assert_eq!(
            d.caret, 18,
            "the end of `three four`, before the space the wrap ate"
        );
        assert_eq!(d.caret_pos(), (1, 10), "drawn on the row it is the end of");
        d.move_end(false);
        assert_eq!(d.caret, 18, "a second End moved the caret");
        d.move_home(false);
        assert_eq!(d.caret, 8, "Home takes the row's own start");
    }

    #[test]
    fn vertical_motion_crosses_a_soft_wrap() {
        // Down aimed at the row below's column 0, an offset that resolved *up*
        // to the row above's end — so it landed on the offset it already had and
        // the caret could never leave a paragraph's first row.
        let mut d = wrapped_doc("wrap_down");
        d.caret = 0;
        for (want, row) in [(8, 1), (19, 2), (28, 3), (39, 3)] {
            d.move_down(false);
            assert_eq!(d.caret, want, "Down stalled");
            assert_eq!(d.caret_pos().0, row, "Down landed on the wrong row");
        }
        d.move_down(false);
        assert_eq!(d.caret, 39, "the last row's Down runs to the end and stops");

        // ...and back up, one row per press. The goal column is the end of the
        // last row, past every other row's width, so each press clamps to the
        // row's own last offset rather than to the one that opens the next.
        let mut d = wrapped_doc("wrap_up");
        d.caret = 39;
        for (want, pos) in [(27, (2, 8)), (18, (1, 10)), (7, (0, 7)), (0, (0, 0))] {
            d.move_up(false);
            assert_eq!(d.caret, want, "Up stalled");
            assert_eq!(d.caret_pos(), pos, "Up landed on the wrong row");
        }
    }

    #[test]
    fn a_kill_on_a_wrapped_row_stops_at_the_row() {
        // The kills take the same line Home and End do, so in WYSIWYG they take
        // the visual row — and a soft wrap has no newline in it to delete, so
        // nothing is joined by reaching the end of one.
        let mut d = wrapped_doc("wrap_kill");
        d.caret = 19; // the `f` of "five", opening the third row
        d.delete_to_line_end();
        // The space the wrap ate goes with the row it was drawn on: sparing it
        // would leave "four  seven", two spaces where the row had been.
        assert_eq!(d.source, "one two three four seven eight");

        // Backwards from the row's last caret position — which is *before* that
        // space, so this one survives, being on the far side of the caret.
        let mut d = wrapped_doc("wrap_kill_back");
        d.caret = 27;
        d.delete_to_line_start();
        assert_eq!(d.source, "one two three four  seven eight");
    }

    // ── document start / end ────────────────────────────────────────────────

    #[test]
    fn move_doc_start_and_end_jump_to_the_edges() {
        let g = |m, f: fn(&mut Doc)| golden("doc_edges", m, f);
        assert_eq!(
            g("hello\nwor|ld\n", |d| d.move_doc_start(false)),
            "|hello\nworld\n"
        );
        assert_eq!(
            g("hel|lo\nworld\n", |d| d.move_doc_end(false)),
            "hello\nworld\n|"
        );
        // Already at the edge: a no-op.
        assert_eq!(g("|hello\n", |d| d.move_doc_start(false)), "|hello\n");
        assert_eq!(g("hello|\n", |d| d.move_doc_end(false)), "hello\n|");
    }

    #[test]
    fn move_doc_start_and_end_extend_the_selection() {
        assert_eq!(
            golden("doc_edges_ext_end", "hello wor|ld\n", |d| d
                .move_doc_end(true)),
            "hello wor[ld\n|]"
        );
        assert_eq!(
            golden("doc_edges_ext_start", "hello wor|ld\n", |d| d
                .move_doc_start(true)),
            "[|hello wor]ld\n"
        );
    }

    #[test]
    fn move_doc_start_and_end_on_an_empty_document_are_a_no_op() {
        let mut d = doc_with("empty_edges", "");
        d.move_doc_end(false);
        assert_eq!(d.caret, 0);
        d.move_doc_start(false);
        assert_eq!(d.caret, 0);
    }

    // ── arrow collapses an active selection ─────────────────────────────────

    #[test]
    fn arrow_collapses_selection_to_its_near_edge() {
        let mut d = doc_with("collapse", "hello world\n");

        // Forward selection (anchor before caret): Right -> end, Left -> start.
        d.anchor = Some(2);
        d.caret = 7;
        d.move_right(false);
        assert_eq!((d.caret, d.anchor), (7, None));

        d.anchor = Some(2);
        d.caret = 7;
        d.move_left(false);
        assert_eq!((d.caret, d.anchor), (2, None));

        // Backward selection (anchor after caret): edges are the same
        // regardless of which end the caret started on.
        d.anchor = Some(7);
        d.caret = 2;
        d.move_right(false);
        assert_eq!((d.caret, d.anchor), (7, None));

        d.anchor = Some(7);
        d.caret = 2;
        d.move_left(false);
        assert_eq!((d.caret, d.anchor), (2, None));
    }

    #[test]
    fn arrow_with_extend_keeps_growing_the_selection() {
        let mut d = doc_with("collapse_extend", "hello world\n");
        d.anchor = Some(2);
        d.caret = 7;
        d.move_right(true); // extend: no collapse, caret steps one further
        assert_eq!((d.caret, d.anchor), (8, Some(2)));
    }

    #[test]
    fn arrow_without_a_selection_moves_one_character_as_before() {
        let mut d = doc_with("no_collapse", "hello\n");
        d.caret = 2;
        d.move_right(false);
        assert_eq!(d.caret, 3);
        d.move_left(false);
        assert_eq!(d.caret, 2);
    }

    /// Press Right until it stops, collecting the offsets walked through. Every
    /// caret bug in the WYSIWYG view shows up here as a walk that ends early:
    /// two stops sharing one source offset can't be moved between, so the caret
    /// stalls on the first of them and the walk never reaches the rest.
    fn walk_right(d: &mut Doc) -> Vec<usize> {
        let mut seen = vec![d.caret];
        for _ in 0..2000 {
            let before = d.caret;
            d.move_right(false);
            if d.caret == before {
                break;
            }
            seen.push(d.caret);
        }
        seen
    }

    #[test]
    fn the_caret_crosses_a_soft_break() {
        // A newline inside a paragraph is a `soft_break`, which twig gives no
        // span of its own — the space it renders as used to borrow the offset of
        // the character before it, and a caret can't move without changing
        // offset. Right must walk clean off the end of the first line.
        let mut d = wysiwyg_doc("soft_break_walk", "one two\nthree four\n");
        d.caret = 0;
        let seen = walk_right(&mut d);
        assert_eq!(seen, (0..=18).collect::<Vec<_>>(), "walk stalled: {seen:?}");
    }

    #[test]
    fn line_flow_preserve_resplits_the_map_and_defaults_to_fold() {
        // The paragraph holds one soft break. Folded (the default) it lays out as
        // a single reflowed row; Preserve re-lays it as a row per source line.
        // The setter must invalidate the cached map for the change to show, and
        // again on the way back — so a round trip returns to the folded layout.
        let mut d = wysiwyg_doc("line_flow", "one two\nthree four\n");
        assert_eq!(d.line_flow(), LineFlow::Fold, "fold is the default");
        d.build_visual(80);
        assert_eq!(d.vmap.num_rows(), 1, "fold: one flowing row");

        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
        assert_eq!(d.vmap.num_rows(), 2, "preserve: a row per source line");

        d.set_line_flow(LineFlow::Fold);
        d.build_visual(80);
        assert_eq!(d.vmap.num_rows(), 1, "fold again: back to one row");
    }

    #[test]
    fn the_caret_still_crosses_a_preserved_soft_break() {
        // Preserve renders the soft break as a row boundary rather than a space,
        // but the caret must still reach every offset — the break's own offset is
        // the first row's end stop, so Right walks clean off the end of line one
        // onto line two, exactly as it does when the break is folded.
        let mut d = wysiwyg_doc("preserve_walk", "one two\nthree four\n");
        d.set_line_flow(LineFlow::Preserve);
        d.build_visual(80);
        d.caret = 0;
        let seen = walk_right(&mut d);
        assert_eq!(seen, (0..=18).collect::<Vec<_>>(), "walk stalled: {seen:?}");
    }

    #[test]
    fn the_caret_walks_a_code_block() {
        // Every glyph of a code block used to map to the block's start, so the
        // whole block was a single offset and the caret couldn't move inside it.
        let src = "```rust\nlet x = 1;\nfn f() {}\n```\n";
        let mut d = wysiwyg_doc("code_walk", src);
        d.caret = 0;
        let seen = walk_right(&mut d);
        // The fences are markup: hidden, and no caret stop. The code between
        // them is reached a character at a time.
        let code = src.find("let").unwrap()..src.find("\n```").unwrap();
        for off in code.clone() {
            assert!(seen.contains(&off), "offset {off} unreachable: {seen:?}");
        }
        assert!(seen.contains(&code.end), "no stop after the last line");
    }

    #[test]
    fn the_caret_walks_an_indented_code_block() {
        // An indented block's text has the four-space indent stripped, so it
        // isn't a verbatim slice and its lines have to be re-found. The caret
        // lands on the code, never in the indent.
        let src = "    indented\n    code\n";
        let mut d = wysiwyg_doc("indent_code_walk", src);
        d.caret = 0;
        let seen = walk_right(&mut d);
        assert!(seen.contains(&src.find("indented").unwrap()));
        assert!(seen.contains(&src.find("code").unwrap()));
        assert!(
            !seen.contains(&0) || seen[0] == 0,
            "the caret starts where it was put"
        );
        // Nothing in the stripped indent is a stop.
        for off in [1, 2, 3] {
            assert!(!seen.contains(&off), "landed in the indent at {off}");
        }
    }

    #[test]
    fn the_caret_leaves_a_tight_heading() {
        // "# H" with text directly under it: the heading row's end and the
        // separator row's end are the same offset. Right used to find the
        // separator's copy, set the caret to where it already was, and stop.
        let mut d = wysiwyg_doc("tight_heading_walk", "# H\ntext\n");
        d.caret = 2; // the "H"
        let seen = walk_right(&mut d);
        assert!(
            seen.len() > 2,
            "Right stalled at the heading's end: {seen:?}"
        );
        assert!(
            seen.contains(&8),
            "never reached the end of \"text\": {seen:?}"
        );
    }

    #[test]
    fn the_caret_skips_the_gap_between_two_paragraphs() {
        // The blank line between two paragraphs is the boundary itself. The
        // caret used to be able to sit on it, and typing there landed in the
        // previous paragraph — "A\n\nB" became "A\nx\nB", one paragraph with a
        // soft break, so the text visibly snapped back up.
        let mut d = wysiwyg_doc("gap_skip", "A\n\nB\n");
        d.caret = 1; // the end of "A"
        d.move_right(false);
        assert_eq!(d.caret, 3, "Right stopped in the gap");
        d.insert("x");
        assert_eq!(d.source, "A\n\nxB\n", "typing landed outside B");
    }

    #[test]
    fn down_from_a_paragraph_lands_on_the_next_one() {
        let mut d = wysiwyg_doc("gap_down", "A\n\nB\n");
        d.caret = 0;
        d.move_down(false);
        assert_eq!(d.caret, 3, "Down stopped in the gap");
    }

    #[test]
    fn clicking_the_gap_lands_on_real_text() {
        // A click can still *reach* the gap — it's drawn, so it's clickable.
        // It has to resolve to somewhere the caret can be.
        let mut d = wysiwyg_doc("gap_click", "A\n\nB\n");
        d.click(1, 0, false); // the gap row
        assert!(
            d.caret == 1 || d.caret == 3,
            "click left the caret in the gap at {}",
            d.caret
        );
        d.insert("x");
        // Either edge of the boundary is a fair place to land; inside it isn't.
        assert!(
            d.source == "Ax\n\nB\n" || d.source == "A\n\nxB\n",
            "click in the gap typed into the boundary: {:?}",
            d.source
        );
    }

    #[test]
    fn enter_opens_an_empty_paragraph_the_caret_can_type_into() {
        // Enter inserts a paragraph break, which leaves a blank line spare on
        // either side of a new one. That middle line is a real empty paragraph:
        // the caret lands there, and typing makes a paragraph rather than
        // extending a neighbour.
        let mut d = wysiwyg_doc("gap_enter", "A\n\nB\n");
        d.caret = 1;
        d.newline();
        assert_eq!(d.source, "A\n\n\n\nB\n");
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(
            d.vmap.row_is_navigable(row),
            "the caret landed on a gap row"
        );
        d.insert("x");
        assert_eq!(
            d.source, "A\n\nx\n\nB\n",
            "the new paragraph merged into a neighbour"
        );
    }

    #[test]
    fn enter_at_the_end_of_the_document_opens_a_paragraph_too() {
        let mut d = wysiwyg_doc("gap_eof", "A\n");
        d.caret = 1;
        d.newline();
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(
            d.vmap.row_is_navigable(row),
            "the caret landed on a gap row"
        );
        d.insert("x");
        assert!(
            d.source.starts_with("A\n\n") && d.source.contains('x'),
            "typing at the end merged into A: {:?}",
            d.source
        );
    }

    // ── click_past_end ───────────────────────────────────────────────────────

    /// [`Doc::click_past_end`] on `body`, and the source it left, with the caret
    /// rendered as `|`.
    fn past_end(name: &str, body: &str) -> (Doc, String) {
        let mut d = wysiwyg_doc(name, body);
        d.click_past_end();
        let out = render_caret(&d);
        (d, out)
    }

    #[test]
    fn click_past_end_opens_an_empty_paragraph_under_the_last_block() {
        let (mut d, out) = past_end("pe_para", "A\n");
        assert_eq!(out, "A\n\n|");
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(
            d.vmap.row_is_navigable(row),
            "the caret landed on a gap row"
        );
        d.insert("x");
        assert_eq!(d.source, "A\n\nx", "typing merged into A");
    }

    #[test]
    fn click_past_end_writes_both_newlines_when_the_file_has_none() {
        assert_eq!(past_end("pe_bare", "A").1, "A\n\n|");
    }

    #[test]
    fn click_past_end_is_only_a_caret_move_when_the_paragraph_is_already_there() {
        let (d, out) = past_end("pe_there", "A\n\n");
        assert_eq!(out, "A\n\n|");
        assert!(!d.dirty, "a click wrote to a document it did not need to");
        assert!(
            !d.can_undo(),
            "a click that changed nothing left an undo step"
        );
    }

    #[test]
    fn click_past_end_leaves_a_closed_fence() {
        let (mut d, out) = past_end("pe_fence", "```\ncode\n```\n");
        assert_eq!(out, "```\ncode\n```\n\n|");
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(!d.vmap.rows[row].code, "the caret is still on a code row");
        d.insert("x");
        assert_eq!(d.source, "```\ncode\n```\n\nx");
    }

    #[test]
    fn click_past_end_closes_an_unclosed_fence_first() {
        assert_eq!(past_end("pe_open", "```\ncode\n").1, "```\ncode\n```\n\n|");
        assert_eq!(
            past_end("pe_open2", "````\ncode").1,
            "````\ncode\n````\n\n|"
        );
        assert_eq!(past_end("pe_tilde", "~~~\ncode\n").1, "~~~\ncode\n~~~\n\n|");
        assert_eq!(
            past_end("pe_quoted", "> ```\n> code\n").1,
            "> ```\n> code\n> ```\n\n|"
        );
    }

    #[test]
    fn click_past_end_does_not_close_an_indented_block() {
        assert_eq!(past_end("pe_indent", "    code\n").1, "    code\n\n|");
    }

    #[test]
    fn click_past_end_under_a_list_and_a_table_leaves_them() {
        let (mut d, out) = past_end("pe_list", "- a\n- b\n");
        assert_eq!(out, "- a\n- b\n\n|");
        d.insert("x");
        assert_eq!(d.source, "- a\n- b\n\nx", "typed into the list");
        assert_eq!(
            past_end("pe_table", "| a |\n|---|\n| b |\n").1,
            "| a |\n|---|\n| b |\n\n|"
        );
    }

    #[test]
    fn click_past_end_on_an_empty_document_writes_nothing() {
        let (d, out) = past_end("pe_empty", "");
        assert_eq!(out, "|");
        assert!(!d.dirty);
    }

    #[test]
    fn click_past_end_is_one_undo_step() {
        let (mut d, _) = past_end("pe_undo", "A\n");
        d.undo();
        assert_eq!(d.source, "A\n");
    }

    #[test]
    fn click_past_end_in_the_source_view_only_moves_the_caret() {
        let mut d = doc_with("pe_source", "A\n");
        d.click_past_end();
        assert_eq!(render_caret(&d), "A\n|");
        assert!(!d.dirty);
    }

    #[test]
    fn click_past_end_on_a_read_only_document_only_moves_the_caret() {
        let mut d = wysiwyg_doc("pe_ro", "A\n");
        d.set_read_only(true);
        d.click_past_end();
        assert_eq!(render_caret(&d), "A\n|");
    }

    // ── toggle_code_block ────────────────────────────────────────────────────

    #[test]
    fn toggle_code_block_fences_the_paragraph_at_the_caret_and_reverses() {
        let g = |n, m| golden_in(View::Wysiwyg, n, m, |d| d.toggle_code_block());
        assert_eq!(g("cb_on", "hel|lo\n"), "```\nhel|lo\n```\n");
        assert_eq!(g("cb_off", "```\nhel|lo\n```\n"), "hel|lo\n");
        assert_eq!(g("cb_end", "hello|\n"), "```\nhello|\n```\n");
        assert_eq!(
            g("cb_wrap", "aaa\nb|bb\nccc\n"),
            "```\naaa\nb|bb\nccc\n```\n"
        );
        assert_eq!(
            g("cb_unwrap", "```\naaa\nb|bb\nccc\n```\n"),
            "aaa\nb|bb\nccc\n"
        );
        // An indented block dedents, and the lines stay one-to-one.
        assert_eq!(g("cb_indent", "    co|de\n"), "co|de\n");
        // A quote's marker is kept on every line, the fence's included.
        assert_eq!(g("cb_quote", "> hel|lo\n"), "> ```\n> hel|lo\n> ```\n");
    }

    #[test]
    fn toggle_code_block_on_a_blank_line_opens_an_empty_fence() {
        let g = |n, m| golden_in(View::Wysiwyg, n, m, |d| d.toggle_code_block());
        assert_eq!(g("cb_blank", "A\n\n|"), "A\n\n```\n|\n```");
        assert_eq!(
            g("cb_blank_mid", "A\n\n|\n\nB\n"),
            "A\n\n```\n|\n```\n\nB\n"
        );
        // Tight against a neighbour, a blank line goes in on that side.
        assert_eq!(g("cb_blank_tight", "A\n|\nB\n"), "A\n\n```\n|\n```\n\nB\n");
        // Inside a quote, on a quoted blank line.
        assert_eq!(
            g("cb_blank_quote", "> A\n>\n> |\n"),
            "> A\n>\n> ```\n> |\n> ```\n"
        );
    }

    #[test]
    fn toggle_code_block_from_a_blank_line_is_a_block_the_caret_can_type_into() {
        let mut d = wysiwyg_doc("cb_type", "A\n\n");
        d.click_past_end();
        d.toggle_code_block();
        assert!(
            d.caret_in_code_block(),
            "the caret is not in the block it opened"
        );
        d.insert("let x = 1;");
        d.newline();
        d.insert("x");
        assert_eq!(d.source, "A\n\n```\nlet x = 1;\nx\n```");
        d.build_visual(80);
        let (row, _) = d.caret_pos();
        assert!(d.vmap.rows[row].code, "typed text is not on a code row");
        // And back out: the button reverses what it did, text kept.
        d.toggle_code_block();
        assert_eq!(d.source, "A\n\nlet x = 1;\nx\n");
        assert!(!d.caret_in_code_block());
    }

    #[test]
    fn toggle_code_block_over_a_selection_fences_it_whole_and_keeps_it_selected() {
        let mut d = wysiwyg_doc("cb_sel", "one\n\ntwo\n\nthree\n");
        d.anchor = Some(1);
        d.caret = 7;
        d.toggle_code_block();
        assert_eq!(d.source, "```\none\n\ntwo\n```\n\nthree\n");
        assert!(d.selection().is_some(), "the block came back unselected");
        d.toggle_code_block();
        assert_eq!(
            d.source, "one\n\ntwo\n\nthree\n",
            "a second press did not reverse the first"
        );
    }

    #[test]
    fn toggle_code_block_inside_a_list_item_fences_and_unfences_in_place() {
        for (name, before, fenced) in [
            ("cb_list", "- it|em\n", "- ```\n  it|em\n  ```\n"),
            ("cb_olist", "1. it|em\n", "1. ```\n   it|em\n   ```\n"),
            ("cb_task", "- [ ] it|em\n", "- [ ] ```\n  it|em\n  ```\n"),
            (
                "cb_nested",
                "- a\n  - it|em\n",
                "- a\n  - ```\n    it|em\n    ```\n",
            ),
        ] {
            let mut d = wysiwyg_doc(name, &before.replace('|', ""));
            d.caret = before.find('|').unwrap();
            d.toggle_code_block();
            assert_eq!(render_caret(&d), fenced, "{name}: fencing");
            assert!(
                d.caret_in_code_block(),
                "{name}: the caret is in the new block"
            );
            d.toggle_code_block();
            assert_eq!(
                render_caret(&d),
                before,
                "{name}: a second press reverses the first"
            );
        }
    }

    #[test]
    fn toggle_code_block_in_a_list_item_is_one_undo_step() {
        let mut d = wysiwyg_doc("cb_list_undo", "- item\n");
        d.caret = 4;
        d.toggle_code_block();
        d.undo();
        assert_eq!(render_caret(&d), "- it|em\n");
    }

    #[test]
    fn caret_in_code_block_reads_fenced_and_indented_blocks() {
        let mut d = wysiwyg_doc("cb_in", "para\n\n```\ncode\n```\n\n    more\n");
        d.caret = 2;
        assert!(!d.caret_in_code_block());
        d.caret = 11;
        assert!(d.caret_in_code_block());
        d.caret = 25;
        assert!(d.caret_in_code_block(), "an indented block is a code block");
    }

    #[test]
    fn caret_in_blockquote_reads_every_line_of_a_quote_and_nothing_after() {
        let src = "para\n\n> # Head\n> body|\n\nafter\n";
        let mut d = wysiwyg_doc("bq_in", &src.replace('|', ""));
        let quote_start = src.find('>').unwrap();
        let body_end = src.find('|').unwrap();
        let after = src.find("after").unwrap() - 1;
        for (at, inside) in [
            (2, false),
            (quote_start + 4, true),
            (body_end - 2, true),
            (body_end, true),
            (after, false),
            (after + 2, false),
        ] {
            d.caret = at;
            assert_eq!(d.caret_in_blockquote(), inside, "caret at {at}");
        }
        // A heading in a quote is both: the quote is a layer over the style.
        d.caret = quote_start + 4;
        assert_eq!(d.current_heading_level(), Some(1));
    }

    #[test]
    fn caret_in_blockquote_follows_the_toggle() {
        let mut d = wysiwyg_doc("bq_toggle", "hello\n");
        d.caret = 2;
        assert!(!d.caret_in_blockquote());
        d.toggle_blockquote();
        assert!(d.caret_in_blockquote());
        d.toggle_blockquote();
        assert!(!d.caret_in_blockquote());
    }

    #[test]
    fn toggle_code_block_is_one_undo_step() {
        let mut d = wysiwyg_doc("cb_undo", "hello\n");
        d.caret = 2;
        d.toggle_code_block();
        d.undo();
        assert_eq!(render_caret(&d), "he|llo\n");
    }

    #[test]
    fn triple_click_selects_a_paragraph_across_its_soft_breaks() {
        // A paragraph broken over two source lines is one paragraph. Selecting
        // it must not stop at the newline inside it — that newline is markup the
        // rich-text view exists to hide.
        let src = "one two\nthree four\n\nnext\n";
        let mut d = wysiwyg_doc("triple_para", src);
        d.select_block_at(2);
        assert_eq!(
            d.selected_text(),
            Some("one two\nthree four"),
            "stopped at the soft break"
        );
    }

    #[test]
    fn the_wheel_can_scroll_away_from_a_caret_that_stays_put() {
        // The reader scrolls down past the caret's row. Nothing moved the
        // caret, so the view must stay where it was put — the old code revealed
        // the caret every frame, which dragged the view straight back and made
        // the document unscrollable past the caret.
        let mut d = wysiwyg_doc("scroll_free", "a\n\nb\n\nc\n\nd\n\ne\n");
        d.caret = 0;
        d.follow_caret(0, 3, 9); // first frame: the caret is at the top
        d.scroll = 4; // the wheel
        d.follow_caret(0, 3, 9);
        assert_eq!(
            d.scroll, 4,
            "the wheel was overruled by a caret that never moved"
        );
    }

    #[test]
    fn moving_the_caret_brings_the_view_back_to_it() {
        let mut d = wysiwyg_doc("scroll_follow", "a\n\nb\n\nc\n\nd\n\ne\n");
        d.caret = 0;
        d.follow_caret(0, 3, 9);
        d.scroll = 6; // scrolled away
        d.move_right(false); // ...and now the caret moves
        let (row, _) = d.caret_pos();
        d.follow_caret(row, 3, 9);
        assert!(
            d.scroll <= row && row < d.scroll + 3,
            "caret row {row} off screen at scroll {}",
            d.scroll
        );
    }

    #[test]
    fn scrolling_stops_at_the_last_row() {
        let mut d = wysiwyg_doc("scroll_clamp", "a\n\nb\n");
        d.caret = 0;
        d.follow_caret(0, 3, 3); // a first frame, so the caret isn't "new"
        d.scroll = 999; // the wheel, spun hard
        d.follow_caret(0, 3, 3);
        assert_eq!(d.scroll, 2, "scrolled into the void past the document");
    }

    #[test]
    fn every_cell_of_a_wide_table_is_reachable() {
        // A table whose cells are far wider than the surface: the columns are
        // cut to fit and the text wraps inside them, so no cell hangs off the
        // right edge where the caret can never go.
        let src = "| Ingredient | Notes |\n|---|---|\n\
                   | flour milled coarse | sift it twice before folding it in |\n";
        let mut d = wysiwyg_doc("wide_table_walk", src);
        d.build_visual(30);
        d.caret = 0;
        let seen = walk_right(&mut d);
        for word in ["Ingredient", "Notes", "coarse", "folding"] {
            let at = src.find(word).unwrap();
            assert!(seen.contains(&at), "{word:?} at {at} unreachable: {seen:?}");
        }
    }

    // ── view parity ──────────────────────────────────────────────────────────
    // `doc_with` pins the source view, so everything above tests a view users
    // never start in — `Doc::open` opens in WYSIWYG. These run the motion and
    // deletion golden cases through *both*, plus the WYSIWYG cases the two
    // can't share: where the source carries markup the rendered text is a
    // different string, and the views agreeing would itself be the bug.

    const VIEWS: [(View, &str); 2] = [(View::Source, "source"), (View::Wysiwyg, "wysiwyg")];

    /// Run `action` in both views on one `|`-marked fixture and assert they
    /// agree. Plain prose only: with no markup to hide, WYSIWYG renders the
    /// source verbatim, so the two views are looking at the same text and any
    /// disagreement is one of them having lost the plot.
    fn both_views(name: &str, marked: &str, action: fn(&mut Doc)) -> String {
        let (src, caret) = parse_caret(marked);
        let run = |view: View, tag: &str| {
            let mut d = doc_in(view, &format!("{name}_{tag}"), &src);
            d.caret = caret;
            action(&mut d);
            render_caret(&d)
        };
        let source = run(VIEWS[0].0, VIEWS[0].1);
        let wysiwyg = run(VIEWS[1].0, VIEWS[1].1);
        assert_eq!(source, wysiwyg, "the views disagree on {marked:?}");
        source
    }

    #[test]
    fn word_motion_agrees_across_the_views_on_plain_prose() {
        let g = both_views;
        assert_eq!(
            g("par_wl", "hello wor|ld", |d| d.move_word_left(false)),
            "hello |world"
        );
        assert_eq!(
            g("par_wl2", "hello| world", |d| d.move_word_left(false)),
            "|hello world"
        );
        assert_eq!(
            g("par_wr", "hel|lo world", |d| d.move_word_right(false)),
            "hello| world"
        );
        assert_eq!(
            g("par_wr2", "hello| world", |d| d.move_word_right(false)),
            "hello world|"
        );
        assert_eq!(
            g("par_punct", "|foo.bar", |d| d.move_word_right(false)),
            "foo|.bar"
        );
        assert_eq!(
            g("par_ext", "hello |world", |d| d.move_word_right(true)),
            "hello [world|]"
        );
    }

    #[test]
    fn word_deletion_agrees_across_the_views_on_plain_prose() {
        let g = both_views;
        assert_eq!(
            g("par_db", "hello world|", |d| d.delete_word_back()),
            "hello |"
        );
        assert_eq!(
            g("par_df", "hello |world", |d| d.delete_word_forward()),
            "hello |"
        );
        assert_eq!(
            g("par_db2", "foo |bar baz", |d| d.delete_word_back()),
            "|bar baz"
        );
        assert_eq!(g("par_utf8", "café |ok", |d| d.delete_word_back()), "|ok");
    }

    #[test]
    fn character_motion_and_deletion_agree_across_the_views_on_plain_prose() {
        let g = both_views;
        assert_eq!(g("par_r", "he|llo", |d| d.move_right(false)), "hel|lo");
        assert_eq!(g("par_l", "he|llo", |d| d.move_left(false)), "h|ello");
        assert_eq!(g("par_bs", "hel|lo", |d| d.backspace()), "he|lo");
        assert_eq!(g("par_del", "hel|lo", |d| d.delete_forward()), "hel|o");
    }

    #[test]
    fn wysiwyg_motion_steps_a_grapheme_cluster_the_way_the_source_view_does() {
        // The reproduction: the stop table was built one stop per `char`, so
        // Right parked the caret 4 bytes into a ZWJ sequence — a place the
        // source view, which steps by grapheme, can't reach and backspace can't
        // survive. The two views must land on the same offset.
        let family = "👨‍👩‍👧"; // three emoji strung together with joiners: one cluster
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("cluster_{tag}"), &format!("a{family}b\n"));
            d.caret = 1;
            d.move_right(false);
            assert_eq!(d.caret, 1 + family.len(), "{tag} parked inside the cluster");

            // ...and the edit that used to sever a joiner off the front of it.
            d.backspace();
            assert_eq!(d.source, "ab\n", "{tag} split the cluster");
            assert_eq!(d.caret, 1);
        }
    }

    #[test]
    fn wysiwyg_motion_treats_a_combining_accent_as_one_character() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("combining_{tag}"), "e\u{0301}x\n");
            d.caret = 0;
            d.move_right(false);
            assert_eq!(
                d.caret,
                "e\u{0301}".len(),
                "{tag} stopped on the combining mark"
            );
        }
    }

    #[test]
    fn no_wysiwyg_motion_can_park_the_caret_inside_a_cluster() {
        // The general form: whatever route the caret takes through a document
        // full of clusters, it never lands between the codepoints of one — so no
        // motion-then-backspace sequence can leave a dangling joiner behind.
        use unicode_segmentation::UnicodeSegmentation;

        let src = "a👨‍👩‍👧b e\u{0301}mo👨‍👩‍👧ji\n\nnext 👩‍🚀 line\n";
        let mut d = wysiwyg_doc("cluster_walk", src);
        d.caret = 0;
        let boundaries: Vec<usize> = src
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(src.len()))
            .collect();
        for off in walk_right(&mut d) {
            assert!(
                boundaries.contains(&off),
                "Right stopped at {off}, inside a grapheme cluster"
            );
        }
    }

    #[test]
    fn wysiwyg_word_motion_stays_out_of_hidden_delimiters() {
        // The reproduction: ⌥→ from inside the opening `**` computed its
        // boundary over the raw source and landed on byte 8 — inside the
        // *closing* `**`, which `caret_pos` draws at column 6, immediately after
        // "bold". The caret drew past the bold word and sat inside it.
        let mut d = wysiwyg_doc("wys_word_delim", "a **bold** c\n");
        d.caret = 2;
        d.move_word_right(false);
        assert!(
            d.vmap.is_stop(d.caret),
            "landed at {}, not a caret stop",
            d.caret
        );
        assert_eq!(d.caret, 10, "should land on the space after \"bold\"");
        // The rendered row is "a bold c": column 6 is the space just past "bold",
        // and now the caret is really there rather than only drawn there.
        assert_eq!(d.caret_pos(), (0, 6));

        // ...and back again: ⌥← returns to the "b", not into the opening `**`.
        d.move_word_left(false);
        assert_eq!(d.caret, 4);
        assert_eq!(d.caret_pos(), (0, 2));
    }

    #[test]
    fn wysiwyg_word_delete_takes_the_markup_with_the_word() {
        // The reproduction: ⌥⌫ from after "bold" walked the raw source, stopped
        // inside the closing `**`, and left "a ** c\n" — delimiters with no
        // opener. Glyph space covers the word alone, which would leave
        // "a **** c": markup wrapped around nothing. The word and the styling
        // that was only ever the word's go together.
        let mut d = wysiwyg_doc("wys_word_del_back", "a **bold** c\n");
        d.caret = 10;
        d.delete_word_back();
        assert_eq!(d.source, "a  c\n");
        assert_eq!(d.caret, 2);

        let mut d = wysiwyg_doc("wys_word_del_fwd", "a **bold** c\n");
        d.caret = 4; // the "b"
        d.delete_word_forward();
        assert_eq!(d.source, "a  c\n");
    }

    #[test]
    fn wysiwyg_word_delete_empties_a_nested_mark_and_a_code_span_too() {
        let src = "a ***bold*** c\n";
        let mut d = wysiwyg_doc("wys_word_del_nest", src);
        d.caret = src.find(" c").unwrap();
        d.delete_word_back();
        assert_eq!(
            d.source, "a  c\n",
            "the emph inside the strong empties it too"
        );

        let src = "a `code` c\n";
        let mut d = wysiwyg_doc("wys_word_del_code", src);
        d.caret = src.find(" c").unwrap();
        d.delete_word_back();
        assert_eq!(d.source, "a  c\n");
    }

    #[test]
    fn wysiwyg_word_delete_keeps_a_mark_that_still_has_text() {
        // Only an *emptied* node goes. Take one word of two and the `**` still
        // has a job to do — over the word that's left, with the space the delete
        // pushed against the opening delimiter moved out in front of it, or the
        // run would be no run at all (`** words**` is literal asterisks — see
        // the mark-edge rule on `splice`).
        let src = "a **two words** c\n";
        let mut d = wysiwyg_doc("wys_word_del_partial", src);
        d.caret = src.find(" words").unwrap();
        d.delete_word_back();
        assert_eq!(d.source, "a  **words** c\n");
    }

    #[test]
    fn source_view_word_motion_still_walks_the_markup() {
        // The other half of the decision: in the source view the `**` are
        // characters like any other — they're on the screen, so word motion has
        // to stop at them and a word-delete has to leave them behind. Only
        // WYSIWYG hides them, so only WYSIWYG steps over them.
        let g = |n, m, f: fn(&mut Doc)| golden(n, m, f);
        assert_eq!(
            g("src_word_motion", "a |**bold** c\n", |d| d
                .move_word_right(false)),
            "a **bold|** c\n"
        );
        // The same caret as the WYSIWYG reproduction, and the opposite outcome:
        // here "a ** c\n" is right, because `bold**` is what's to the left of it.
        assert_eq!(
            g("src_word_del", "a **bold**| c\n", |d| d.delete_word_back()),
            "a **| c\n"
        );
    }

    #[test]
    fn every_wysiwyg_motion_lands_on_a_caret_stop() {
        // The single invariant both bugs violated: the caret draws and edits at
        // the same place only when it's on a stop. `debug_assert_on_a_stop`
        // makes the same claim in-place; this pins it from the outside, over a
        // document with every kind of thing the map has to be careful about.
        // At two widths: the wide one every other test builds at, where no
        // fixture folds, and one narrow enough that they all do. A soft wrap is
        // where an offset stops being on exactly one row, and testing only the
        // width that never wraps is how the caret came to be pinned at the first
        // one Down reached.
        let src = "# Title\n\na **bold** e\u{0301}mo👨‍👩‍👧ji `x` c\n\n\
                   - item one\n\n| A | B |\n|---|---|\n| x | y |\n";
        // A table of named operations, which is what it looks like.
        #[allow(clippy::type_complexity)]
        let motions: [(&str, fn(&mut Doc)); 8] = [
            ("right", |d| d.move_right(false)),
            ("left", |d| d.move_left(false)),
            ("word_right", |d| d.move_word_right(false)),
            ("word_left", |d| d.move_word_left(false)),
            ("down", |d| d.move_down(false)),
            ("up", |d| d.move_up(false)),
            ("home", |d| d.move_home(false)),
            ("end", |d| d.move_end(false)),
        ];
        for width in [80, 12] {
            let mut d = wysiwyg_doc("stop_invariant", src);
            d.build_visual(width);
            let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
            assert!(stops.len() > 20, "fixture should have plenty of stops");
            for start in stops {
                for (name, motion) in &motions {
                    d.caret = start;
                    d.anchor = None;
                    motion(&mut d);
                    assert!(
                        d.vmap.is_stop(d.caret),
                        "{name} from {start} at width {width} landed at {} — not a caret stop",
                        d.caret
                    );
                }
            }
        }
    }

    #[test]
    fn no_wysiwyg_motion_is_a_dead_end() {
        // Down held to the bottom of a document reaches the bottom, and Up held
        // to the top reaches the top — from anywhere, at a width that wraps. The
        // invariant above says a motion lands somewhere legal; this one says it
        // gets somewhere at all, which is what a caret pinned at a wrap boundary
        // was quietly failing to do while every assertion around it held.
        let src = "# Title\n\none two three four five six seven eight nine ten\n\n\
                   - item one two three four five\n\nlast\n";
        for width in [80, 12] {
            let mut d = wysiwyg_doc("no_dead_end", src);
            d.build_visual(width);
            let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
            let (first, last) = (stops[0], stops[stops.len() - 1]);
            for &start in &stops {
                for (name, motion, want) in [
                    (
                        "down",
                        (|d: &mut Doc| d.move_down(false)) as fn(&mut Doc),
                        last,
                    ),
                    ("up", |d: &mut Doc| d.move_up(false), first),
                ] {
                    d.caret = start;
                    d.anchor = None;
                    d.goal_col = None;
                    // Every row, plus the presses the edges take, plus slack.
                    for _ in 0..d.vmap.num_rows() + 4 {
                        motion(&mut d);
                    }
                    assert_eq!(
                        d.caret, want,
                        "{name} held from {start} at width {width} never arrived"
                    );
                }
            }
        }
    }
    // ── display columns ──────────────────────────────────────────────────────
    // A `col` is a terminal cell, not a character. The two are the same number
    // for the ASCII the fixtures above are written in, which is how they came
    // apart in the first place: `你` is one character drawn in two cells, so a
    // column counted in characters names a cell the text isn't in — one earlier
    // for every wide character to its left.

    #[test]
    fn a_wide_character_is_two_columns_wide() {
        // The reproduction: `你` is one char and two cells, so the caret just
        // past it drew at column 1 — inside the character it had already left.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("wide_col_{tag}"), "你好\n");
            d.caret = "你".len();
            assert_eq!(d.caret_pos(), (0, 2), "{tag}: caret drew inside 你");
            d.caret = "你好".len();
            assert_eq!(d.caret_pos(), (0, 4), "{tag}");
        }
    }

    #[test]
    fn a_cluster_is_as_wide_as_it_is_drawn_not_as_its_codepoints_measure() {
        // `👨‍👩‍👧` is five codepoints — two-cell, joiner, two-cell, joiner,
        // two-cell — measuring six cells one at a time, but the character they
        // spell is drawn in two. Width belongs to the cluster, not the glyph,
        // and the frontends measure it the same way.
        let family = "👨‍👩‍👧";
        for (view, tag) in VIEWS {
            let src = format!("a{family}b\n");
            let mut d = doc_in(view, &format!("wide_cluster_{tag}"), &src);
            d.caret = 1 + family.len();
            assert_eq!(
                d.caret_pos(),
                (0, 3),
                "{tag}: 'a' is one cell, the family two"
            );
        }
    }

    #[test]
    fn both_cells_of_a_wide_character_mean_the_character() {
        // Clicking the far half of `好` is still clicking `好`: half a character
        // is not a place the caret can be, so it comes to rest at the
        // character's start — the column it would have been drawn at anyway.
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("wide_click_{tag}"), "你好\n");
            for col in [2, 3] {
                d.caret = 0;
                d.click(0, col, false);
                assert_eq!(d.caret, "你".len(), "{tag}: click at col {col}");
                assert_eq!(d.caret_pos(), (0, 2), "{tag}: click at col {col}");
            }
            // Past the last cell is the line's end, as it is for ASCII.
            d.click(0, 9, false);
            assert_eq!(d.caret, "你好".len(), "{tag}: click past the end");
        }
    }

    #[test]
    fn every_offset_survives_the_trip_out_to_a_column_and_back() {
        // The mapping is only a mapping if it inverts: the cell the caret is
        // drawn in has to be the cell that brings it back to the same offset.
        // Over a fixture where a character may be one cell or two, and one
        // codepoint or five.
        use unicode_segmentation::UnicodeSegmentation;

        let src = "ab 你好 c\n\n👨‍👩‍👧 e\u{0301}x 漢字\n\nplain ascii\n";

        let mut d = doc_in(View::Source, "roundtrip_source", src);
        // Every offset the source view's caret can occupy: it steps by grapheme
        // cluster, so those are its boundaries.
        for (off, _) in src
            .grapheme_indices(true)
            .chain(std::iter::once((src.len(), "")))
        {
            d.caret = off;
            let (row, col) = d.caret_pos();
            d.click(row, col, false);
            assert_eq!(d.caret, off, "source: {off} → ({row}, {col}) → {}", d.caret);
        }

        // And in WYSIWYG, where the offsets the caret can occupy are the map's
        // stops rather than every boundary.
        let mut d = doc_in(View::Wysiwyg, "roundtrip_wysiwyg", src);
        let stops: Vec<usize> = (0..=src.len()).filter(|&o| d.vmap.is_stop(o)).collect();
        assert!(stops.len() > 20, "fixture should have plenty of stops");
        for off in stops {
            d.caret = off;
            let (row, col) = d.caret_pos();
            d.click(row, col, false);
            assert_eq!(
                d.caret, off,
                "wysiwyg: {off} → ({row}, {col}) → {}",
                d.caret
            );
        }
    }

    #[test]
    fn vertical_motion_aims_at_a_column_the_reader_can_see() {
        // Down from under `世` lands under the glyph in that cell, not two
        // characters further along the line. The goal is a column, so a line of
        // wide characters and a line of ASCII line up the way they're drawn.
        //
        // The gap differs by view: a bare newline inside a paragraph is a soft
        // break, which WYSIWYG draws as a space on a single row. The views share
        // a grid only where the source's lines are the renderer's rows too.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("你好世{gap}abcdef\n");
            let mut d = doc_in(view, &format!("goal_wide_{tag}"), &src);
            d.caret = "你好".len();
            assert_eq!(d.caret_pos().1, 4, "{tag}: `世` is drawn at column 4");
            d.move_down(false);
            assert_eq!(d.caret_pos().1, 4, "{tag}: goal column lost");
            assert!(
                d.source[d.caret..].starts_with('e'),
                "{tag}: landed on the wrong glyph"
            );
        }
    }

    #[test]
    fn a_goal_column_landing_inside_a_wide_character_lands_on_it() {
        // Down from column 3 onto `你好`, whose characters start at columns 0
        // and 2: column 3 is the *second* cell of `好`. There is nowhere to be
        // between the cells of one character, so the caret rests on it — and on
        // its start, which is the only offset there that is a caret stop.
        for (view, tag) in VIEWS {
            let gap = if view == View::Source { "\n" } else { "\n\n" };
            let src = format!("abcdef{gap}你好\n");
            let mut d = doc_in(view, &format!("goal_inside_{tag}"), &src);
            let line = src.find('你').unwrap();
            d.caret = 3;
            d.move_down(false);
            assert_eq!(d.caret, line + "你".len(), "{tag}: landed off `好`'s start");
            assert_eq!(d.caret_pos().1, 2, "{tag}: drew between `好`'s cells");
        }
    }

    #[test]
    fn a_caret_in_a_table_cell_of_wide_text_draws_where_the_text_is() {
        // The column the cell's text is laid out in is measured in cells, so the
        // caret walking that text has to be too — the two agreeing is the whole
        // point of the grid staying square.
        let mut d = wysiwyg_doc("table_wide", "| A | B |\n|---|---|\n| 你好 | y |\n");
        let at = d.source.find("你").unwrap();
        d.caret = at;
        let (row, col) = d.caret_pos();
        // `│ ` opens the row, so the cell's text starts at column 2; `好` is two
        // cells further along.
        assert_eq!(col, 2, "the cell's first character");
        d.move_right(false);
        assert_eq!(
            d.caret_pos(),
            (row, 4),
            "`好` is drawn past `你`'s two cells"
        );
        assert_eq!(d.caret, at + "你".len());
    }

    // ── active inline marks ───────────────────────────────────────────────────

    /// The marks at a `|`-marked fixture's caret, in `InlineMarks::iter` order.
    fn marks(view: View, name: &str, marked: &str) -> Vec<InlineKind> {
        let (src, caret) = parse_caret(marked);
        let mut d = doc_in(view, name, &src);
        d.caret = caret;
        d.active_inline_marks().iter().collect()
    }

    /// The marks over the selection `[start, end)`.
    fn marks_over(view: View, name: &str, src: &str, start: usize, end: usize) -> Vec<InlineKind> {
        let mut d = doc_in(view, name, src);
        d.anchor = Some(start);
        d.caret = end;
        d.active_inline_marks().iter().collect()
    }

    #[test]
    fn a_caret_in_a_mark_reports_it() {
        for (view, tag) in VIEWS {
            let m = |marked| marks(view, &format!("marks_in_{tag}"), marked);
            assert_eq!(m("a **bo|ld** b"), [InlineKind::Strong], "{tag}");
            assert_eq!(m("a *it|alic* b"), [InlineKind::Emph], "{tag}");
            assert_eq!(m("a `co|de` b"), [InlineKind::Verbatim], "{tag}");
            // Plain text under no mark lights nothing — the toolbar's resting state.
            assert_eq!(m("a| **bold** b"), [], "{tag}");
            assert!(m("plain t|ext").is_empty(), "{tag}");
        }
    }

    #[test]
    fn nested_marks_all_report() {
        // Bold *and* italic: a toolbar lights both buttons, so the set has both —
        // the ancestor chain is a chain, and every mark on it is in force.
        for (view, tag) in VIEWS {
            assert_eq!(
                marks(
                    view,
                    &format!("marks_nested_{tag}"),
                    "**bold and *bo|th*** end"
                ),
                [InlineKind::Strong, InlineKind::Emph],
                "{tag}"
            );
        }
    }

    #[test]
    fn the_caret_at_a_marks_edge_reports_it_where_typing_would_extend_it() {
        // The offsets a WYSIWYG caret actually reaches at a bold run's edges are
        // the first byte of its text and the byte after its last — both inside
        // the mark's span, both places typing lands inside the bold. The offset
        // past the closing delimiter is the next text, and reports nothing.
        let src = "a **bold** b";
        let inner_start = src.find("bold").unwrap(); // 4
        let inner_end = inner_start + "bold".len(); // 8, on the closing `**`
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("marks_edge_{tag}"), src);
            for off in [2, 3, inner_start, inner_end, 9] {
                d.caret = off;
                assert!(
                    d.active_inline_marks().contains(InlineKind::Strong),
                    "{tag}: offset {off} is inside the strong span"
                );
            }
            for off in [0, 1, 10, 11, 12] {
                d.caret = off;
                assert!(
                    !d.active_inline_marks().contains(InlineKind::Strong),
                    "{tag}: offset {off} is outside the strong run"
                );
            }
        }
    }

    #[test]
    fn a_mark_ends_the_same_way_at_the_end_of_the_buffer_as_in_the_middle() {
        // Regression: twig resolves an offset that is one node's end and the
        // next one's start to the node that *starts* there, so `**bold**|\n`
        // isn't bold. With nothing following there's no tie to break and the
        // chain still ended at the mark, which made a trailing `\n` — not the
        // text — decide whether the caret after a bold word reported bold. It's
        // the offset past the mark either way, and typing there is plain either
        // way. A blank document typed into is exactly this shape.
        for (view, tag) in VIEWS {
            let m = |name: String, marked| marks(view, &name, marked);
            assert_eq!(
                m(format!("marks_eob_{tag}"), "**bold**|"),
                [],
                "{tag}: no trailing newline"
            );
            assert_eq!(
                m(format!("marks_eol_{tag}"), "**bold**|\n"),
                [],
                "{tag}: with one"
            );
            // And the last offset that *is* in the mark still is.
            assert_eq!(
                m(format!("marks_eob_in_{tag}"), "**bold*|*"),
                [InlineKind::Strong],
                "{tag}"
            );
        }
    }

    #[test]
    fn a_selection_reports_a_mark_only_when_it_covers_the_whole_thing() {
        let src = "a **bold** b";
        let (b, d_) = (src.find("bold").unwrap(), src.find("bold").unwrap() + 4);
        for (view, tag) in VIEWS {
            let m = |s, e| marks_over(view, &format!("marks_sel_{tag}"), src, s, e);
            // The whole bold word, and a slice of it.
            assert_eq!(m(b, d_), [InlineKind::Strong], "{tag}: the whole word");
            assert_eq!(m(b + 1, d_ - 1), [InlineKind::Strong], "{tag}: a slice");
            // Ending exactly at the closing delimiter's start is still all-bold:
            // an exclusive end sits *past* the last selected character, so the
            // question is asked of the character, not the boundary.
            assert_eq!(
                m(b, d_ + 2),
                [InlineKind::Strong],
                "{tag}: through the close"
            );
            // Half in, half out: Bold lit here would claim a press turns it off.
            assert_eq!(m(0, d_), [], "{tag}: leading plain text");
            assert_eq!(m(b, src.len()), [], "{tag}: trailing plain text");
        }
    }

    #[test]
    fn a_selection_across_two_runs_of_the_same_mark_reports_nothing() {
        // Both ends are bold, but the space between them isn't — two runs are two
        // nodes, which is exactly what the node id catches and a kind-only
        // comparison would not.
        let src = "**one** **two**";
        for (view, tag) in VIEWS {
            let m = marks_over(view, &format!("marks_runs_{tag}"), src, 2, 13);
            assert_eq!(m, [], "{tag}: `one** **two` is not all bold");
        }
    }

    #[test]
    fn marks_read_the_document_as_it_is_edited() {
        // The point of asking twig every frame instead of caching: the answer has
        // to follow the toggle that changed it.
        let mut d = wysiwyg_doc("marks_live", "one two\n");
        d.anchor = Some(0);
        d.caret = 3;
        assert!(d.active_inline_marks().is_empty(), "plain to start");
        d.toggle(InlineKind::Strong);
        assert_eq!(d.source, "**one** two\n");
        // `toggle` leaves the bolded text selected, so the button it lit stays lit.
        assert!(d.active_inline_marks().contains(InlineKind::Strong));
        d.toggle(InlineKind::Strong);
        assert!(d.active_inline_marks().is_empty(), "and off again");
    }

    #[test]
    fn a_link_is_not_an_inline_mark() {
        // `link`/`str` are inline nodes, but nothing on the inline toolbar
        // toggles them — a set with a "link mark" in it would have no button.
        for (view, tag) in VIEWS {
            assert_eq!(
                marks(view, &format!("marks_link_{tag}"), "a [te|xt](u) b"),
                [],
                "{tag}"
            );
        }
    }

    // ── blank documents ───────────────────────────────────────────────────────

    #[test]
    fn a_blank_document_is_untitled_empty_and_markdown() {
        let mut d = Doc::blank().unwrap();
        assert!(d.is_untitled());
        assert_eq!(d.path, PathBuf::new());
        assert_eq!(
            d.file_name(),
            "untitled",
            "the header has to show something"
        );
        assert_eq!(d.format_name(), "markdown");
        assert_eq!(d.source, "");
        assert!(!d.dirty, "nothing typed yet is nothing to lose");
        assert_eq!(d.disk_state(), DiskState::Untitled);
        // And it's a document you can be in: the default view renders it.
        d.build_visual(80);
        assert_eq!(d.caret, 0);
    }

    #[test]
    fn saving_an_untitled_document_asks_for_a_name_instead_of_writing() {
        let mut d = Doc::blank().unwrap();
        d.insert("hello");
        assert!(d.dirty);
        d.save();
        assert_eq!(d.status.as_deref(), Some("untitled — save as…"));
        assert!(d.dirty, "it must not come away believing it saved");
        assert!(d.is_untitled(), "and it still has no file");
    }

    #[test]
    fn a_blank_document_becomes_a_real_one_at_the_first_save_as() {
        let p = temp_path("blank_save_as");
        let mut d = Doc::blank().unwrap();
        // Plain text — a blank doc opens in Hidden mode, where a typed `#` would
        // be kept literal (`\#`); this test is about save-as, not escaping (which
        // has its own test), so it types nothing that escaping would touch.
        d.insert("hi");
        d.save_as(p.clone());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi");
        assert!(!d.is_untitled());
        assert!(!d.dirty);
        assert_eq!(d.file_name(), p.file_name().unwrap().to_string_lossy());
        assert_eq!(
            d.disk_state(),
            DiskState::Unchanged,
            "the watermark is stamped"
        );
        // And ⌘S is a plain save from here on.
        d.insert("!");
        d.save();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hi!");
        let _ = std::fs::remove_file(&p);
    }

    // ── a file that isn't there yet ───────────────────────────────────────────

    /// A unique path in the temp dir with the given extension, guaranteed not to
    /// exist — what `leaf notes.md` is handed when the file has never been made.
    fn missing_path(name: &str, ext: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_test_new_{name}_{seq}.{ext}"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn a_file_that_doesnt_exist_opens_as_an_empty_named_document() {
        let p = missing_path("named", "md");
        let mut d = Doc::open_or_create(p.clone()).unwrap();

        assert_eq!(d.source, "", "nothing was read, so there's nothing in it");
        assert!(!d.dirty, "an untouched new buffer has nothing to lose");
        assert!(
            !d.is_untitled(),
            "it has the name the user asked for — ^S must not detour to Save As"
        );
        assert_eq!(d.file_name(), p.file_name().unwrap().to_str().unwrap());
        assert!(d.path.is_absolute(), "the same absolute path `open` stores");
        assert!(!p.exists(), "and opening it wrote nothing");
        // And it's a document you can be in.
        d.build_visual(80);
        assert_eq!(d.caret, 0);
    }

    #[test]
    fn a_new_file_is_created_by_its_first_save() {
        let p = missing_path("first_save", "md");
        let mut d = Doc::open_or_create(p.clone()).unwrap();
        d.insert("hello\n");
        assert!(d.dirty);
        d.save();

        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "hello\n",
            "a plain ^S wrote it — no Save As, no name to invent"
        );
        assert!(!d.dirty);
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_new_file_takes_its_format_from_the_extension() {
        // The one thing `blank` can't do: with no name it has to assume Markdown,
        // and typing djot into a Markdown parse is the wrong buffer.
        let dj = missing_path("format", "dj");
        assert_eq!(Doc::open_or_create(dj).unwrap().format_name(), "djot");
        let md = missing_path("format", "md");
        assert_eq!(Doc::open_or_create(md).unwrap().format_name(), "markdown");
    }

    #[test]
    fn a_new_file_reports_itself_missing_until_it_is_saved() {
        // Not `Untitled` — that's the answer for a document with no path, and it
        // would tell a frontend there is nothing a save could collide with. Here
        // there is a path, and the file simply isn't at it yet.
        let p = missing_path("disk_state", "md");
        let mut d = Doc::open_or_create(p.clone()).unwrap();
        assert_eq!(d.disk_state(), DiskState::Missing);

        // Somebody else creates it while the buffer is open: that's an overwrite
        // the frontend has to be able to prompt about, exactly as for an opened
        // file. Their bytes, not ours, so `Changed`.
        std::fs::write(&p, "theirs\n").unwrap();
        assert_eq!(d.disk_state(), DiskState::Changed);

        // Saving makes the file ours and re-stamps the watermark.
        d.insert("ours\n");
        d.save();
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "ours\n");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn open_or_create_still_opens_a_file_that_is_there() {
        let d = doc_with("open_or_create_existing", "body\n");
        let reopened = Doc::open_or_create(d.path.clone()).unwrap();
        assert_eq!(reopened.source, "body\n");
        assert_eq!(reopened.disk_state(), DiskState::Unchanged);
    }

    #[test]
    fn a_missing_file_with_no_readable_extension_is_still_an_error() {
        // A mistyped flag or a stray argument must not become a buffer promising
        // to save somewhere — the same refusal `open` gives a real file.
        let mut p = std::env::temp_dir();
        p.push("leaf_test_new_bad_ext.wat");
        assert!(Doc::open_or_create(p).is_err());
        let mut none = std::env::temp_dir();
        none.push("leaf_test_new_no_ext");
        assert!(Doc::open_or_create(none).is_err());
    }

    #[test]
    fn a_new_file_in_a_directory_that_doesnt_exist_opens_but_wont_save() {
        // Opening reads nothing, so there is nothing to fail on yet; the write is
        // where it fails, and it says so rather than claiming a save.
        let p = std::env::temp_dir().join("leaf_test_no_such_dir_c41/doc.md");
        let mut d = Doc::open_or_create(p).unwrap();
        d.insert("x");
        d.save();
        assert!(
            d.status.as_deref().unwrap().starts_with("save failed:"),
            "got {:?}",
            d.status
        );
        assert!(d.dirty, "it must not come away believing it saved");
    }

    // ── save as ───────────────────────────────────────────────────────────────

    /// A unique path in the temp dir that no fixture wrote — a Save As target.
    fn temp_path(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!("leaf_test_target_{name}_{seq}.md"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn save_as_moves_the_document_and_leaves_the_old_file_alone() {
        let mut d = doc_with("save_as_move", "original\n");
        let old = d.path.clone();
        let new = temp_path("save_as_move");
        d.insert("edited: ");
        d.save_as(new.clone());

        assert_eq!(std::fs::read_to_string(&new).unwrap(), "edited: original\n");
        assert_eq!(
            std::fs::read_to_string(&old).unwrap(),
            "original\n",
            "Save As doesn't touch the file it came from"
        );
        assert_eq!(d.path, new, "the document moved");
        assert!(!d.dirty);
        assert_eq!(
            d.status.as_deref(),
            Some(&*format!("saved {}", d.file_name()))
        );

        // Every later save follows it, which is the whole difference from a copy.
        d.caret = 0;
        d.insert("re-");
        d.save();
        assert_eq!(
            std::fs::read_to_string(&new).unwrap(),
            "re-edited: original\n"
        );
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "original\n");
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn save_as_overwrites_an_existing_target() {
        // The picker already asked; asking again down here is the same question
        // twice, and the second one has no way to be answered.
        let new = temp_path("save_as_over");
        std::fs::write(&new, "theirs\n").unwrap();
        let mut d = doc_with("save_as_over", "ours\n");
        d.save_as(new.clone());
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "ours\n");
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn a_save_as_that_fails_leaves_the_document_where_it_was() {
        let mut d = doc_with("save_as_fail", "body\n");
        let old = d.path.clone();
        d.insert("x");
        // A directory that doesn't exist: the write can't land.
        let bad = std::env::temp_dir().join("leaf_test_no_such_dir_9f2/doc.md");
        d.save_as(bad);

        assert_eq!(
            d.path, old,
            "the document must not move to a file that isn't there"
        );
        assert!(d.dirty, "and must not believe it saved");
        assert!(
            d.status.as_deref().unwrap().starts_with("save failed:"),
            "the same failure a plain save reports, got {:?}",
            d.status
        );
        // The original is still the document's file, and still saveable.
        d.save();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "xbody\n");
        assert!(!d.dirty);
    }

    #[test]
    fn save_as_renames_without_reparsing_the_format() {
        // `.dj` on the name doesn't make the buffer djot: it was parsed as
        // Markdown and still is, and saying otherwise would be a conversion the
        // user never asked for (and an undo history thrown away to do it).
        let mut d = doc_with("save_as_format", "**b**\n");
        let mut new = temp_path("save_as_format");
        new.set_extension("dj");
        d.save_as(new.clone());
        assert_eq!(d.format_name(), "markdown");
        let _ = std::fs::remove_file(&new);
    }

    // ── external change / reload ──────────────────────────────────────────────

    #[test]
    fn an_untouched_file_reports_unchanged() {
        let mut d = doc_with("disk_clean", "body\n");
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        // Editing the buffer is not editing the file.
        d.insert("x");
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        assert!(d.dirty);
        // Saving re-stamps the watermark rather than reporting our own bytes back.
        d.save();
        assert_eq!(d.disk_state(), DiskState::Unchanged);
    }

    #[test]
    fn a_file_written_underneath_reports_changed() {
        let mut d = doc_with("disk_changed", "body\n");
        std::fs::write(&d.path, "someone else\n").unwrap();
        assert_eq!(d.disk_state(), DiskState::Changed);
        // Dirty *and* changed is the clobber: both halves are readable, and
        // leaf-core takes neither side.
        d.insert("x");
        assert!(d.dirty && d.disk_state() == DiskState::Changed);
        // Saving anyway is allowed — the frontend asked, or chose not to.
        d.save();
        assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "xbody\n");
        assert_eq!(d.disk_state(), DiskState::Unchanged);
    }

    #[test]
    fn a_file_rewritten_with_the_same_bytes_is_unchanged() {
        // The hash is what makes this honest: the file was written (a fresh
        // mtime), and nothing about the document is stale.
        let d = doc_with("disk_same_bytes", "body\n");
        std::fs::write(&d.path, "body\n").unwrap();
        assert_eq!(d.disk_state(), DiskState::Unchanged);
    }

    #[test]
    fn a_deleted_file_reports_missing() {
        let mut d = doc_with("disk_missing", "body\n");
        std::fs::remove_file(&d.path).unwrap();
        assert_eq!(d.disk_state(), DiskState::Missing);
        // A save recreates it, and the document is whole again.
        d.save();
        assert_eq!(d.disk_state(), DiskState::Unchanged);
        assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "body\n");
    }

    #[test]
    fn reload_replaces_the_document_with_the_file() {
        for (view, tag) in VIEWS {
            let mut d = doc_in(view, &format!("reload_{tag}"), "one\n\ntwo\n");
            d.insert("edited ");
            assert!(d.dirty);
            std::fs::write(&d.path, "one\n\ntwo\n\nthree\n").unwrap();
            d.reload();

            assert_eq!(d.source, "one\n\ntwo\n\nthree\n", "{tag}");
            assert!(!d.dirty, "{tag}: the file is what we have");
            assert_eq!(d.disk_state(), DiskState::Unchanged, "{tag}");
            assert_eq!(
                d.status.as_deref(),
                Some(&*format!("reloaded {}", d.file_name()))
            );
            // The reloaded tree is live, not the old parse.
            d.caret = d.source.find("three").unwrap();
            assert_eq!(d.breadcrumb(), "doc › para › str", "{tag}");
        }
    }

    #[test]
    fn reload_clamps_the_caret_and_drops_the_selection() {
        let mut d = doc_with("reload_caret", "a long first line\n");
        d.caret = 12;
        d.anchor = Some(4);
        std::fs::write(&d.path, "short\n").unwrap();
        d.reload();
        assert_eq!(d.caret, d.source.len(), "clamped into the shorter file");
        assert_eq!(
            d.anchor, None,
            "a selection over bytes that changed is a lie"
        );
        assert!(d.selection().is_none());

        // A caret the file still has room for stays put.
        let mut d = doc_with("reload_caret_keep", "one\n\ntwo\n");
        d.caret = 2;
        std::fs::write(&d.path, "one\n\ntwo\n\nthree\n").unwrap();
        d.reload();
        assert_eq!(d.caret, 2);
    }

    /// A silent reload is something that happened *to* a reader — a formatter,
    /// a `git checkout` — so it has to be undoable like anything else that
    /// changes the document, and undoable as one step rather than as however
    /// many the file happens to differ by.
    #[test]
    fn reload_is_one_undo_step_and_keeps_the_history_under_it() {
        let mut d = doc_with("reload_undo", "body\n");
        d.insert("x");
        assert_eq!(d.source, "xbody\n");
        std::fs::write(&d.path, "replaced\n").unwrap();
        d.reload();
        assert_eq!(d.source, "replaced\n");
        assert!(!d.dirty, "a reload lands clean");

        // One ^Z takes the whole swap off, and hands back the unsaved work it
        // replaced — which is unsaved again, because the file no longer says it.
        d.undo();
        assert_eq!(d.source, "xbody\n", "the reload comes off in one step");
        assert!(d.dirty, "and what it comes back to is unsaved");
        // …and the history under it is still there.
        d.undo();
        assert_eq!(
            d.source, "body\n",
            "the typing before the reload undoes too"
        );
        // Redo walks back up through the reload.
        d.redo();
        d.redo();
        assert_eq!(d.source, "replaced\n");
    }

    /// A file rewritten with the bytes it already had is not an edit, so it
    /// must not leave an undo step behind for something nobody did.
    #[test]
    fn reloading_identical_bytes_pushes_no_undo_step() {
        let mut d = doc_with("reload_same", "body\n");
        d.insert("x");
        std::fs::write(&d.path, "xbody\n").unwrap();
        d.reload();
        assert_eq!(d.source, "xbody\n");
        assert!(!d.dirty, "the file now says what the buffer does");
        d.undo();
        assert_eq!(
            d.source, "body\n",
            "one step back is the typing, not a no-op"
        );
    }

    #[test]
    fn a_reload_that_cant_read_leaves_the_document_alone() {
        let mut d = doc_with("reload_gone", "body\n");
        d.insert("x");
        std::fs::remove_file(&d.path).unwrap();
        d.reload();
        assert_eq!(d.source, "xbody\n", "the unsaved work is still here");
        assert!(d.dirty);
        assert!(
            d.status.as_deref().unwrap().starts_with("reload failed:"),
            "{:?}",
            d.status
        );

        // And an untitled document has nothing to reload from.
        let mut d = Doc::blank().unwrap();
        d.insert("typed");
        d.reload();
        assert_eq!(d.source, "typed");
        assert_eq!(d.status.as_deref(), Some("no file to reload"));
    }

    #[test]
    fn a_read_only_document_refuses_every_door() {
        let mut d = doc_with("readonly", "one two three\n");
        d.insert("x");
        assert!(d.dirty, "writable first, so the undo step exists");
        d.set_read_only(true);
        let before = d.source.clone();
        d.insert("y");
        d.backspace();
        d.undo();
        d.redo();
        assert_eq!(d.source, before, "no door moved a byte");
        d.set_read_only(false);
        d.undo();
        assert_ne!(d.source, before, "off again, the same doors work");
    }

    /// The doors that go to twig's own verbs rather than through the splice.
    /// Typed text in the rendered view under the default markup mode is the
    /// everyday one — it is what a keystroke in leaf-web or the Apple views
    /// becomes — and it walked straight past the gate.
    #[test]
    fn a_read_only_document_refuses_the_doors_around_the_splice() {
        let mut d = wysiwyg_doc(
            "readonly-doors",
            "one two three\n\n| a | b |\n|---|---|\n| c | d |\n",
        );
        d.set_markup_mode(MarkupMode::None);
        d.set_read_only(true);
        let before = d.source.clone();
        d.place_caret(3, false);
        d.insert("y");
        d.insert_link("https://example.com");
        d.insert_image("a.png", "alt");
        d.insert_thematic_break();
        d.insert_footnote();
        d.place_caret(0, false);
        d.place_caret(3, true);
        d.toggle(InlineKind::Strong);
        d.toggle_heading(2);
        d.set_block(BlockKind::Paragraph);
        d.toggle_list(false);
        d.toggle_blockquote();
        d.toggle_task_item();
        d.newline();
        d.indent();
        d.set_code_language("rust");
        let in_cell = d.source.find("| c").unwrap() + 2;
        d.place_caret(in_cell, false);
        assert!(d.caret_in_table(), "the caret is in the grid");
        assert!(!d.cell_line_break(), "the cell break reports the refusal");
        assert_eq!(d.source, before, "no door moved a byte");
        assert!(!d.dirty, "nothing to save");
        d.set_read_only(false);
        d.place_caret(3, false);
        d.insert("y");
        assert_ne!(d.source, before, "off again, the same doors work");
    }

    #[test]
    fn a_selection_quote_carries_its_context_on_char_boundaries() {
        let mut d = doc_with("quote", "before 你好 exact 世界 after\n");
        let start = d.source.find("exact").unwrap();
        d.place_caret(start, false);
        d.place_caret(start + "exact".len(), true);
        let q = d.selection_quote(3).unwrap();
        assert_eq!(q.exact, "exact");
        assert_eq!(
            q.prefix, "你好 ",
            "chars, not bytes — the multibyte pair counts as two"
        );
        assert_eq!(q.suffix, " 世界");
        assert_eq!(&d.source[q.start..q.end], "exact");
        // At the edges the context clips rather than erring.
        d.place_caret(0, false);
        d.place_caret(6, true);
        let q = d.selection_quote(40).unwrap();
        assert_eq!(q.prefix, "");
        assert_eq!(q.exact, "before");
        // No selection is no quote.
        d.place_caret(0, false);
        assert!(d.selection_quote(3).is_none());
    }

    #[test]
    fn highlights_are_kept_sorted_and_answer_point_queries() {
        let mut d = doc_with("hl", "one two three\n");
        d.set_highlights(vec![
            Highlight {
                start: 8,
                end: 13,
                id: "b".into(),
                color: None,
                marker: None,
            },
            Highlight {
                start: 0,
                end: 3,
                id: "a".into(),
                color: Some("#ffe066".into()),
                marker: None,
            },
            Highlight {
                start: 5,
                end: 5,
                id: "empty".into(),
                color: None,
                marker: None,
            },
        ]);
        assert_eq!(
            d.highlights()
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"],
            "sorted by start, the empty range dropped"
        );
        assert_eq!(d.highlight_at(1).map(|h| h.id.as_str()), Some("a"));
        assert_eq!(d.highlight_at(3), None, "end is exclusive");
        assert_eq!(d.highlight_at(8).map(|h| h.id.as_str()), Some("b"));
        d.set_highlights(Vec::new());
        assert!(d.highlights().is_empty(), "a replace is a replace");
    }

    /// `Highlight::covering` and the cursor over it are what both painters ask
    /// per glyph, so they have to answer the same as the scan they replaced —
    /// including in the gaps, which is where most glyphs are.
    #[test]
    fn covering_answers_from_a_sorted_list_without_scanning_all_of_it() {
        let hl = |start: usize, end: usize, id: &str| Highlight {
            start,
            end,
            id: id.into(),
            color: None,
            marker: None,
        };
        // Disjoint, as search hits are: in a range, in a gap, and past the end.
        let hits: Vec<Highlight> = (0..20).map(|i| hl(i * 10, i * 10 + 3, "hit")).collect();
        assert_eq!(Highlight::covering(&hits, 0).map(|h| h.start), Some(0));
        assert_eq!(Highlight::covering(&hits, 102).map(|h| h.start), Some(100));
        assert_eq!(
            Highlight::covering(&hits, 105),
            None,
            "a gap covers nothing"
        );
        assert_eq!(Highlight::covering(&hits, 103), None, "end is exclusive");
        assert_eq!(Highlight::covering(&hits, 9_999), None);
        assert_eq!(Highlight::covering(&[], 0), None);

        // Nested: first by start, so a hit inside an annotation still resolves
        // to the annotation — and the range that stops short doesn't mask it.
        let nested = vec![hl(0, 20, "outer"), hl(5, 10, "inner")];
        assert_eq!(
            Highlight::covering(&nested, 7).map(|h| h.id.as_str()),
            Some("outer")
        );
        assert_eq!(
            Highlight::covering(&nested, 15).map(|h| h.id.as_str()),
            Some("outer")
        );
    }

    /// The cursor is an optimisation, so the only thing worth asserting is that
    /// it is not also a change of answer — at every offset, over a list with a
    /// nest in it, walked forwards and then backwards.
    #[test]
    fn the_highlight_cursor_answers_exactly_what_a_fresh_scan_would() {
        let hl = |start: usize, end: usize, id: &str| Highlight {
            start,
            end,
            id: id.into(),
            color: None,
            marker: None,
        };
        let mut list = vec![
            hl(0, 20, "outer"),
            hl(5, 10, "inner"),
            hl(30, 33, "hit"),
            hl(40, 43, "hit"),
        ];
        list.sort_by_key(|h| (h.start, h.end));

        let mut cursor = HighlightCursor::new(&list);
        for offset in 0..50 {
            assert_eq!(
                cursor.at(offset).map(|h| h.id.as_str()),
                Highlight::covering(&list, offset).map(|h| h.id.as_str()),
                "cursor disagrees at {offset}"
            );
        }
        // Backwards: the cursor re-seats rather than answering from where it
        // had got to, so a painter that revisits a row is still told the truth.
        for offset in (0..50).rev() {
            assert_eq!(
                cursor.at(offset).map(|h| h.id.as_str()),
                Highlight::covering(&list, offset).map(|h| h.id.as_str()),
                "cursor disagrees walking back at {offset}"
            );
        }
    }

    // ── the presentation vocabulary ─────────────────────────────────────────

    /// A document in `format`, for the gesture tests that want more than the
    /// Markdown `doc_with` writes.
    fn fmt_doc(body: &str, format: Format) -> Doc {
        Doc::from_source(body.to_string(), format).unwrap()
    }

    /// Alignment is a block property, so the gesture is `set_block_attrs` on
    /// the caret's block whatever is selected — and each format spells it its
    /// own way: djot's `{…}` line above the block, a `<div>` around it in
    /// Markdown (the format has nowhere else to put it), the tag in HTML.
    #[test]
    fn set_alignment_spells_the_class_the_format_s_own_way() {
        let mut dj = fmt_doc("hello\n", Format::Djot);
        dj.caret = 1;
        dj.set_alignment(Some(Align::Center));
        assert_eq!(dj.source, "{.center}\nhello\n");
        assert!(dj.dirty);
        assert_eq!(dj.status, None);

        let mut md = fmt_doc("hello\n", Format::Markdown);
        md.caret = 1;
        md.set_alignment(Some(Align::Right));
        assert_eq!(md.source, "<div class=\"right\">\n\nhello\n\n</div>\n");

        let mut html = fmt_doc("<p>hello</p>\n", Format::Html);
        html.caret = html.source.find("hello").unwrap();
        html.set_alignment(Some(Align::Justify));
        assert_eq!(html.source, "<p class=\"justify\">hello</p>\n");
    }

    /// Each gesture edits **one key and keeps the rest** — twig's contract is
    /// replace-not-merge, so leaf reads the node's attributes, edits its own
    /// key out of them, and passes the list back whole. A document from
    /// elsewhere passes through the editor unharmed.
    #[test]
    fn a_presentation_gesture_keeps_every_attribute_it_did_not_write() {
        let mut d = fmt_doc(
            "{.lead .center #intro data-line-height=\"1.5\"}\nhello\n",
            Format::Djot,
        );
        d.caret = d.source.find("hello").unwrap();
        d.set_alignment(Some(Align::Right));
        // `center` goes, `lead` stays, and neither the id nor the spacing is
        // touched.
        // The serializer picks the order; what matters is which keys survive.
        assert!(d.source.contains(".lead"), "{:?}", d.source);
        assert!(d.source.contains(".right"), "{:?}", d.source);
        assert!(!d.source.contains(".center"), "{:?}", d.source);
        assert!(d.source.contains("#intro"), "{:?}", d.source);
        assert!(
            d.source.contains("data-line-height=\"1.5\""),
            "{:?}",
            d.source
        );
        assert_eq!(d.alignment_at_caret(), Some(Align::Right));
        assert_eq!(
            d.line_spacing_at_caret(),
            Some(LineHeight::Step(LineSpacing::OneHalf))
        );

        // And the other way round: the spacing gesture leaves the classes be.
        d.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
        assert!(d.source.contains(".lead"), "{:?}", d.source);
        assert!(d.source.contains(".right"), "{:?}", d.source);
        assert_eq!(
            d.line_spacing_at_caret(),
            Some(LineHeight::Step(LineSpacing::Double))
        );
    }

    /// Clearing is the same gesture with `None`: the key goes, the tokens leaf
    /// owns go out of `class`, and a block left with nothing at all is spelled
    /// bare again — in Markdown by unwrapping the div twig wrapped it in.
    #[test]
    fn none_clears_a_key_and_an_empty_set_unwraps_the_block() {
        let mut dj = fmt_doc("{.lead .center}\nhello\n", Format::Djot);
        dj.caret = dj.source.find("hello").unwrap();
        dj.set_alignment(None);
        assert_eq!(dj.source, "{.lead}\nhello\n", "the foreign class stays");
        assert_eq!(dj.alignment_at_caret(), None);

        let mut bare = fmt_doc("{.center}\nhello\n", Format::Djot);
        bare.caret = bare.source.find("hello").unwrap();
        bare.set_alignment(None);
        assert_eq!(
            bare.source, "hello\n",
            "the last key takes the line with it"
        );

        let mut md = fmt_doc("hello\n", Format::Markdown);
        md.caret = 1;
        md.set_alignment(Some(Align::Center));
        assert_eq!(md.source, "<div class=\"center\">\n\nhello\n\n</div>\n");
        md.caret = md.source.find("hello").unwrap();
        md.set_line_spacing(Some(LineHeight::Step(LineSpacing::OneFifteen)));
        assert_eq!(
            md.source, "<div class=\"center\" data-line-height=\"1.15\">\n\nhello\n\n</div>\n",
            "the second key rewrites the div rather than nesting a second"
        );
        md.caret = md.source.find("hello").unwrap();
        md.set_alignment(None);
        md.caret = md.source.find("hello").unwrap();
        md.set_line_spacing(None);
        assert_eq!(md.source, "hello\n", "an empty set unwraps the div");
    }

    /// Size, face and colour are the run's over a selection and the block's
    /// with none — so "make this paragraph larger" is a click with the caret in
    /// it rather than a select-all first.
    #[test]
    fn a_run_gesture_wraps_a_selection_and_sets_the_block_without_one() {
        // With a selection: a span, in each format's own spelling.
        let mut dj = fmt_doc("a big b\n", Format::Djot);
        dj.anchor = Some(2);
        dj.caret = 5;
        dj.set_font_size(Some(FontSize::Step(SizeStep::Large)));
        assert_eq!(dj.source, "a [big]{data-size=\"large\"} b\n");
        assert_eq!(
            dj.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Large))
        );

        let mut md = fmt_doc("a big b\n", Format::Markdown);
        md.anchor = Some(2);
        md.caret = 5;
        md.set_text_color(Some(TextColor::Named(MarkColor::Blue)));
        assert_eq!(md.source, "a <span data-color=\"blue\">big</span> b\n");
        assert_eq!(
            md.text_color_at_caret(),
            Some(TextColor::Named(MarkColor::Blue))
        );

        // Without one: the caret's block, through the block gesture.
        let mut block = fmt_doc("a big b\n", Format::Djot);
        block.caret = 3;
        block.set_font_family(Some(FontFace::Generic(FontFamily::Monospace)));
        assert_eq!(block.source, "{data-font=\"monospace\"}\na big b\n");
        assert_eq!(
            block.font_family_at_caret(),
            Some(FontFace::Generic(FontFamily::Monospace))
        );
    }

    /// The *Other…* row of each of the four menus: a value goes into the
    /// document in its canonical spelling and comes back out of the query as
    /// the same value. One round trip per property, because the four go out
    /// through different doors — two block gestures, and the run three through
    /// the span that `wrap_range_attrs` mints.
    #[test]
    fn an_exact_value_round_trips_through_the_gesture_and_the_query() {
        // Size: the run three, over a selection.
        let mut d = fmt_doc("a big b\n", Format::Djot);
        d.anchor = Some(2);
        d.caret = 5;
        d.set_font_size(FontSize::points(14.0));
        assert_eq!(d.source, "a [big]{data-size=\"14pt\"} b\n");
        assert_eq!(d.font_size_at_caret(), FontSize::points(14.0));

        // Colour, onto the same span — the gesture keeps the size it finds.
        d.set_text_color(Some(TextColor::Rgb {
            r: 0xc0,
            g: 0x30,
            b: 0x30,
        }));
        assert_eq!(
            d.source,
            "a [big]{data-size=\"14pt\" data-color=\"#c03030\"} b\n"
        );
        assert_eq!(
            d.text_color_at_caret(),
            Some(TextColor::Rgb {
                r: 0xc0,
                g: 0x30,
                b: 0x30
            })
        );

        // Face: a family name, as given.
        d.set_font_family(Some(FontFace::Named("Garamond".into())));
        assert!(
            d.source.contains("data-font=\"Garamond\""),
            "{:?}",
            d.source
        );
        assert_eq!(
            d.font_family_at_caret(),
            Some(FontFace::Named("Garamond".into()))
        );

        // Line spacing: a block gesture, and an exact ratio.
        let mut block = fmt_doc("hello\n", Format::Djot);
        block.caret = 1;
        block.set_line_spacing(LineHeight::ratio(1.3));
        assert_eq!(block.source, "{data-line-height=\"1.3\"}\nhello\n");
        assert_eq!(block.line_spacing_at_caret(), LineHeight::ratio(1.3));

        // And a value spelled long is written back short, so the same press
        // twice writes the same bytes: `14.0pt` in, `14pt` out.
        let mut long = fmt_doc("{data-size=\"14.0pt\"}\nhello\n", Format::Djot);
        long.caret = long.source.find("hello").unwrap();
        assert_eq!(long.font_size_at_caret(), FontSize::points(14.0));
        let in_force = long.font_size_at_caret();
        long.set_font_size(in_force);
        assert_eq!(long.source, "{data-size=\"14pt\"}\nhello\n");
    }

    /// A value the grammar does not cover is what it was before the vocabulary
    /// opened: carried untouched by the document, answered `None` by the query
    /// so the menu ticks *Default*, and rewritten only by a gesture on its own
    /// key. leaf is not going to grow a CSS parser to guess at `1.3em`.
    #[test]
    fn a_value_outside_the_grammar_is_carried_and_the_menu_ticks_the_default() {
        let src =
            "{data-size=\"huge\" data-color=\"rgb(1,2,3)\" data-line-height=\"1.3em\"}\nhello\n";
        let mut d = fmt_doc(src, Format::Djot);
        d.caret = d.source.find("hello").unwrap();
        assert_eq!(d.font_size_at_caret(), None);
        assert_eq!(d.text_color_at_caret(), None);
        assert_eq!(d.line_spacing_at_caret(), None);

        // The keys are still there, untouched, after a gesture on a *different*
        // key — "edit one key and keep the rest" holds for a value it cannot
        // read as readily as for one it can.
        d.set_alignment(Some(Align::Center));
        assert!(d.source.contains("data-size=\"huge\""), "{:?}", d.source);
        assert!(
            d.source.contains("data-color=\"rgb(1,2,3)\""),
            "{:?}",
            d.source
        );
        assert!(
            d.source.contains("data-line-height=\"1.3em\""),
            "{:?}",
            d.source
        );
        // And the gesture on its *own* key replaces it, which is the one way a
        // carried value ever changes.
        d.caret = d.source.find("hello").unwrap();
        d.set_font_size(FontSize::points(12.0));
        assert!(d.source.contains("data-size=\"12pt\""), "{:?}", d.source);
        assert!(!d.source.contains("huge"), "{:?}", d.source);
    }

    /// The nearest node wins whichever *form* either node wrote: a value inside
    /// a name, a name inside a value. The fold has one rule and does not learn
    /// a second one for exact values.
    #[test]
    fn the_nearest_node_wins_whether_it_named_a_size_or_measured_one() {
        // A value inside a name: the block says `small`, the span says `14pt`.
        let mut d = fmt_doc(
            "{data-size=\"small\"}\nx [y]{data-size=\"14pt\"} z\n",
            Format::Djot,
        );
        d.caret = d.source.find('y').unwrap();
        assert_eq!(d.font_size_at_caret(), FontSize::points(14.0));
        d.caret = d.source.find('x').unwrap();
        assert_eq!(
            d.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Small))
        );

        // And a name inside a value, which is the same rule read the other way.
        let mut e = fmt_doc(
            "{data-size=\"14pt\" data-color=\"#c03030\"}\nx [y]{data-size=\"small\"} z\n",
            Format::Djot,
        );
        e.caret = e.source.find('y').unwrap();
        assert_eq!(
            e.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Small))
        );
        assert_eq!(
            e.text_color_at_caret(),
            Some(TextColor::Rgb {
                r: 0xc0,
                g: 0x30,
                b: 0x30
            }),
            "the block's colour still reaches the span"
        );
        e.caret = e.source.find('x').unwrap();
        assert_eq!(e.font_size_at_caret(), FontSize::points(14.0));
    }

    /// twig re-styles the span a range already lies in rather than nesting a
    /// second, and an empty set unwraps it — so a second press of the menu
    /// fixes the size instead of building `[[big]{.a}]{.b}`, and the entry that
    /// means "the theme's own" takes the span away.
    #[test]
    fn a_second_run_gesture_re_styles_the_span_and_none_unwraps_it() {
        let mut d = fmt_doc("a big b\n", Format::Djot);
        d.anchor = Some(2);
        d.caret = 5;
        d.set_font_size(Some(FontSize::Step(SizeStep::Large)));
        assert_eq!(d.source, "a [big]{data-size=\"large\"} b\n");

        // The selection `wrap_range_attrs` left behind covers the whole span;
        // colouring it now keeps the size, because the gesture reads the span's
        // attributes before it edits its own key.
        d.set_text_color(Some(TextColor::Named(MarkColor::Red)));
        assert_eq!(
            d.source, "a [big]{data-size=\"large\" data-color=\"red\"} b\n",
            "one span, both keys"
        );
        assert_eq!(
            d.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Large))
        );
        assert_eq!(
            d.text_color_at_caret(),
            Some(TextColor::Named(MarkColor::Red))
        );

        d.set_text_color(None);
        assert_eq!(d.source, "a [big]{data-size=\"large\"} b\n");
        d.set_font_size(None);
        assert_eq!(d.source, "a big b\n", "the last key unwraps the span");
        assert_eq!(d.font_size_at_caret(), None);
    }

    /// The queries read the nearest node that names the property: the span the
    /// caret is in, then its block, then the `div`s around it.
    #[test]
    fn a_presentation_query_reads_the_nearest_node_that_names_it() {
        let mut d = fmt_doc(
            "{.center data-size=\"small\" data-font=\"serif\"}\nx [y]{data-size=\"xx-large\"} z\n",
            Format::Djot,
        );
        // In the span: its own size, the block's face and alignment.
        d.caret = d.source.find('y').unwrap();
        assert_eq!(
            d.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::XxLarge))
        );
        assert_eq!(
            d.font_family_at_caret(),
            Some(FontFace::Generic(FontFamily::Serif))
        );
        assert_eq!(d.alignment_at_caret(), Some(Align::Center));
        assert_eq!(d.line_spacing_at_caret(), None);
        assert_eq!(d.text_color_at_caret(), None);

        // Outside it: the block's size.
        d.caret = d.source.find('x').unwrap();
        assert_eq!(
            d.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Small))
        );

        // And through a Markdown div, which is where a Markdown block's
        // attributes live.
        let mut md = fmt_doc(
            "<div class=\"center\" data-size=\"large\">\n\nhello\n\n</div>\n",
            Format::Markdown,
        );
        md.caret = md.source.find("hello").unwrap();
        assert_eq!(md.alignment_at_caret(), Some(Align::Center));
        assert_eq!(
            md.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Large))
        );

        // A document that names none of it answers `None` everywhere, which is
        // "the theme's own" and what every toolbar draws unlit.
        let mut plain = doc_with("plain_presentation", "hello\n");
        plain.caret = 1;
        assert_eq!(plain.alignment_at_caret(), None);
        assert_eq!(plain.line_spacing_at_caret(), None);
        assert_eq!(plain.font_size_at_caret(), None);
        assert_eq!(plain.font_family_at_caret(), None);
        assert_eq!(plain.text_color_at_caret(), None);
    }

    /// A djot fenced div is anonymous the way an attributed span is, and is a
    /// block all the same — the *form* is the whole of what tells them apart.
    /// Read as a span it poisoned both halves: the run gesture copied the div's
    /// entire attribute set onto the span it minted, duplicating the `id`, and
    /// the run and block queries answered off a node the walker draws nothing
    /// for.
    #[test]
    fn a_djot_fenced_div_is_not_an_attributed_span() {
        let src = "{.center data-size=\"small\" #box}\n:::\nhello world\n:::\n";
        let mut d = fmt_doc(src, Format::Djot);
        let at = d.source.find("world").unwrap();
        d.anchor = Some(at);
        d.caret = at + "world".len();
        d.set_text_color(Some(TextColor::Named(MarkColor::Red)));
        assert_eq!(
            d.source,
            "{.center data-size=\"small\" #box}\n:::\nhello [world]{data-color=\"red\"}\n:::\n",
            "the span carries its own key and nothing of the div's"
        );

        // And the queries stop at the block: a djot div is not a `<div>`, the
        // walker lends its keys to nothing inside it, and a query that said
        // otherwise would tick a menu entry no glyph on screen obeys.
        assert_eq!(
            d.text_color_at_caret(),
            Some(TextColor::Named(MarkColor::Red))
        );
        assert_eq!(d.font_size_at_caret(), None);
        assert_eq!(d.alignment_at_caret(), None);
    }

    /// Clearing a property the block does not name and a `div` around it does
    /// would write nothing and change nothing — twig's `set_block_attrs`
    /// reaches one node, and the div is not it. The gesture says so instead of
    /// leaving the author pressing an entry that never ticks.
    #[test]
    fn clearing_a_property_an_enclosing_div_names_says_so_and_writes_nothing() {
        // Markdown, two paragraphs in one div: not the sole-child shape twig
        // writes, so `block_attrs_at_caret` reads the paragraph and the
        // paragraph names none of it.
        let src = "<div class=\"center\" data-line-height=\"1.5\" data-size=\"large\">\n\nhello\n\nworld\n\n</div>\n";
        let mut md = fmt_doc(src, Format::Markdown);
        md.caret = md.source.find("hello").unwrap();
        assert_eq!(md.alignment_at_caret(), Some(Align::Center));

        md.set_alignment(None);
        assert_eq!(md.source, src, "nothing written");
        assert!(!md.dirty);
        assert_eq!(
            md.status.as_deref(),
            Some("alignment: set on the div around the block")
        );
        assert_eq!(md.alignment_at_caret(), Some(Align::Center));

        // The same for a `data-` key, at both levels — the block pair and the
        // run three, the run three at a bare caret being the block gesture.
        md.set_line_spacing(None);
        assert_eq!(md.source, src);
        assert_eq!(
            md.status.as_deref(),
            Some("line spacing: set on the div around the block")
        );
        md.set_font_size(None);
        assert_eq!(md.source, src);
        assert_eq!(
            md.status.as_deref(),
            Some("size: set on the div around the block")
        );

        // HTML has no sole-child fold at all: a block's attributes go on the
        // block, so the div around one is always out of reach.
        let html_src = "<div class=\"center\"><p>hi</p></div>\n";
        let mut html = fmt_doc(html_src, Format::Html);
        html.caret = html.source.find("hi").unwrap();
        assert_eq!(html.alignment_at_caret(), Some(Align::Center));
        html.set_alignment(None);
        assert_eq!(html.source, html_src);
        assert!(!html.dirty);
        assert_eq!(
            html.status.as_deref(),
            Some("alignment: set on the div around the block")
        );

        // And it is a refusal, not a rule against clearing: a block that names
        // the property itself still loses it, div or no div.
        let mut own = fmt_doc(
            "<div class=\"center\"><p class=\"right\">hi</p></div>\n",
            Format::Html,
        );
        own.caret = own.source.find("hi").unwrap();
        own.set_alignment(None);
        assert_eq!(own.source, "<div class=\"center\"><p>hi</p></div>\n");
        assert_eq!(own.status, None);
    }

    /// An edited key is rewritten **where it stands**. The proposal's worked
    /// example is the test: a paragraph that came in as `id="intro"
    /// class="lead center" data-line-height="1.5"` and is right-aligned goes
    /// out as the same list with one token changed. Removing the key and
    /// pushing it back shuffled a document's attributes on every press.
    #[test]
    fn an_edited_key_keeps_its_place_among_the_attributes() {
        let mut html = fmt_doc(
            "<p id=\"intro\" class=\"lead center\" data-line-height=\"1.5\">hello</p>\n",
            Format::Html,
        );
        html.caret = html.source.find("hello").unwrap();
        html.set_alignment(Some(Align::Right));
        assert_eq!(
            html.source,
            "<p id=\"intro\" class=\"lead right\" data-line-height=\"1.5\">hello</p>\n"
        );

        // A `data-` key the same way, and a key the block did not have still
        // goes on the end.
        html.caret = html.source.find("hello").unwrap();
        html.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
        assert_eq!(
            html.source,
            "<p id=\"intro\" class=\"lead right\" data-line-height=\"2\">hello</p>\n"
        );
        html.caret = html.source.find("hello").unwrap();
        html.set_font_size(Some(FontSize::Step(SizeStep::Large)));
        assert_eq!(
            html.source,
            "<p id=\"intro\" class=\"lead right\" data-line-height=\"2\" data-size=\"large\">hello</p>\n"
        );

        // Djot writes the same list in its own spelling, and the order is the
        // author's there too.
        let mut dj = fmt_doc(
            "{#intro .lead .center data-line-height=\"1.5\"}\nhello\n",
            Format::Djot,
        );
        dj.caret = dj.source.find("hello").unwrap();
        dj.set_alignment(Some(Align::Right));
        assert_eq!(
            dj.source,
            "{#intro .lead .right data-line-height=\"1.5\"}\nhello\n"
        );
    }

    /// A page break is a block, so twig alone lands one after the caret's whole
    /// block; the paragraph is parted at the caret first, exactly as
    /// `insert_thematic_break` parts it, and each format spells the directive
    /// its own way.
    #[test]
    fn insert_page_break_parts_the_paragraph_and_spells_the_directive() {
        let mut md = doc_with("page_break_md", "hello world\n");
        md.caret = 5;
        md.insert_page_break();
        assert_eq!(md.source, "hello\n\n::page-break\n\nworld\n");
        assert!(md.dirty);
        assert_eq!(md.status, None);

        let mut dj = fmt_doc("hello world\n", Format::Djot);
        dj.caret = 5;
        dj.insert_page_break();
        assert_eq!(dj.source, "hello\n\n::: page-break\n:::\n\nworld\n");

        // At a block's end there is no second half to mint, so the break simply
        // follows the block — the rule the rule button already has.
        let mut end = doc_with("page_break_end", "hello\n");
        end.caret = 5;
        end.insert_page_break();
        assert_eq!(end.source, "hello\n\n::page-break\n\n");
        assert_eq!(end.caret, end.source.len(), "on the line under the break");

        // And it reaches the map as the placeholder row a frontend paginates on.
        end.view = View::Wysiwyg;
        end.build_visual(80);
        assert_eq!(
            end.vmap
                .rows
                .iter()
                .find_map(|r| r.leaf_directive.as_ref())
                .map(|m| m.name.as_str()),
            Some(PAGE_BREAK)
        );

        // HTML and AsciiDoc spell it their own way, and draw it the same.
        for (fmt, src, spelled) in [
            (Format::Html, "<p>hello</p>\n", "<page-break></page-break>"),
            (Format::Asciidoc, "hello\n", "<<<"),
        ] {
            let mut d = fmt_doc(src, fmt);
            d.caret = d.source.find("hello").unwrap() + 5;
            d.insert_page_break();
            assert!(d.source.contains(spelled), "{fmt:?}: {:?}", d.source);
            d.view = View::Wysiwyg;
            d.build_visual(80);
            let marks = d
                .vmap
                .rows
                .iter()
                .filter_map(|r| r.leaf_directive.as_ref())
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(marks, [PAGE_BREAK], "{fmt:?}");
        }
    }

    /// The general gesture a host's catalogue calls: a directive named by the
    /// host, with a label and attributes, written where the caret parts the
    /// paragraph, and read back into the map with all three — the way the
    /// view hands it to the host that draws it.
    #[test]
    fn insert_directive_round_trips_name_label_and_attrs() {
        let attrs: Attrs = vec![
            ("src".into(), Some("https://x.org/a".into())),
            ("title".into(), Some("a \"quoted\" b".into())),
            ("wide".into(), None),
        ];
        let mut md = doc_with("directive_md", "hello world\n");
        md.caret = 5;
        md.insert_directive("x-card", Some("A card"), &attrs);
        assert_eq!(md.status, None);
        assert_eq!(
            md.source,
            "hello\n\n::x-card[A card]{src=\"https://x.org/a\" title=\"a \\\"quoted\\\" b\" wide}\n\nworld\n"
        );
        assert!(md.dirty);
        md.view = View::Wysiwyg;
        md.build_visual(80);
        let [d] = md.vmap.directives.as_slice() else {
            panic!("one directive, got {:?}", md.vmap.directives)
        };
        assert_eq!(d.name, "x-card");
        assert_eq!(d.label, "A card");
        // A bare attribute reads back with an empty value: that is what twig
        // reports for Markdown's `{wide}`.
        assert_eq!(
            d.key().attrs,
            [
                ("src".to_string(), "https://x.org/a".to_string()),
                ("title".to_string(), "a \"quoted\" b".to_string()),
                ("wide".to_string(), String::new()),
            ]
        );
        assert_eq!(d.attr("src"), Some("https://x.org/a"));

        // djot: the attributes on a line of their own over the fence, a bare
        // one spelled `key=""` so that djot reads it as an attribute at all,
        // and a class kept apart from the name the fence gives.
        let mut dj = fmt_doc("hello world\n", Format::Djot);
        dj.caret = 5;
        let dj_attrs: Attrs = vec![
            ("class".into(), Some("wide".into())),
            ("src".into(), Some("https://x.org/a".into())),
            ("flag".into(), None),
        ];
        dj.insert_directive("x-card", None, &dj_attrs);
        assert_eq!(dj.status, None);
        assert_eq!(
            dj.source,
            "hello\n\n{.wide src=\"https://x.org/a\" flag=\"\"}\n::: x-card\n:::\n\nworld\n"
        );
        dj.view = View::Wysiwyg;
        dj.build_visual(80);
        let [d] = dj.vmap.directives.as_slice() else {
            panic!("one directive, got {:?}", dj.vmap.directives)
        };
        assert_eq!(
            d.name, "x-card",
            "the fence's word, not the attribute line's class"
        );
        assert_eq!(d.label, "");
        assert_eq!(d.attr("class"), Some("wide"));
        assert_eq!(d.attr("src"), Some("https://x.org/a"));
        assert_eq!(d.attr("flag"), Some(""));
    }

    /// `insert_page_break` is `insert_directive` with leaf's own name: the
    /// same bytes, the same caret, the same one undo step.
    #[test]
    fn insert_directive_places_the_caret_as_a_page_break_does() {
        for (fmt, body, caret) in [
            (Format::Markdown, "a\n\nb\n", 1),
            (Format::Markdown, "a\n", 1),
            (Format::Markdown, "hello world\n", 5),
            (Format::Djot, "a\n\nb\n", 1),
            (Format::Djot, "hello world\n", 5),
        ] {
            let run = |f: &dyn Fn(&mut Doc)| {
                let mut d = Doc::from_source(body.into(), fmt).unwrap();
                d.view = View::Wysiwyg;
                d.caret = caret;
                f(&mut d);
                (d.source.clone(), d.caret)
            };
            assert_eq!(
                run(&|d| d.insert_page_break()),
                run(&|d| d.insert_directive(PAGE_BREAK, None, &[])),
                "{fmt:?} {body:?}"
            );
        }

        // And a host's name lands the caret on a line under it, typed into
        // there, and taken back in one step with that line.
        let mut d = Doc::from_source("a\n\nb\n".into(), Format::Markdown).unwrap();
        d.view = View::Wysiwyg;
        d.caret = 1;
        d.insert_directive("x-card", Some("A"), &[("k".into(), Some("v".into()))]);
        assert_eq!(d.source, "a\n\n::x-card[A]{k=v}\n\n\n\nb\n");
        d.build_visual(80);
        let (row, col) = d.vmap.pos_of_offset(d.caret);
        assert_eq!(col, 0);
        assert!(d.vmap.rows[row].glyphs.is_empty(), "an empty line");
        assert!(d.vmap.rows[row - 2].leaf_directive.is_some(), "under it");
        d.insert("X");
        assert_eq!(d.source, "a\n\n::x-card[A]{k=v}\n\nX\n\nb\n");
        d.undo();
        d.undo();
        assert_eq!(
            d.source, "a\n\nb\n",
            "the directive and its line are one step"
        );
    }

    /// A name or a label twig will not write is refused before anything is
    /// touched — the selection it would have replaced and the paragraph it
    /// would have parted are both left as they were.
    #[test]
    fn insert_directive_refuses_a_bad_name_or_label_and_writes_nothing() {
        let src = "hello world\n";
        for (fmt, name, label) in [
            (Format::Markdown, "1x", None),
            (Format::Markdown, "a b", None),
            (Format::Markdown, "a:b", None),
            (Format::Markdown, "", None),
            (Format::Markdown, "ok", Some("a]b")),
            (Format::Markdown, "ok", Some("a\nb")),
            (Format::Djot, "a b", None),
            // djot has nowhere for a label to go.
            (Format::Djot, "ok", Some("A card")),
        ] {
            let mut d = fmt_doc(src, fmt);
            d.anchor = Some(2);
            d.caret = 5;
            d.insert_directive(name, label, &[]);
            assert_eq!(d.source, src, "{fmt:?} {name:?} {label:?}");
            assert!(!d.dirty, "{fmt:?} {name:?} {label:?}");
            assert!(!d.can_undo(), "{fmt:?} {name:?} {label:?}");
            let status = d.status.as_deref().unwrap_or("");
            assert!(
                status.starts_with("directive: "),
                "{fmt:?} {name:?} {label:?}: {status:?}"
            );
        }
    }

    /// `directives` is claimed only where what is written comes back as a
    /// directive with its name. twig spells one in HTML and AsciiDoc too — and
    /// `page_break` is true there — but the walker reads their spellings of
    /// `page-break` alone, so an arbitrary name would be written and then
    /// drawn as something else, or not at all.
    #[test]
    fn directives_are_offered_only_where_the_walker_reads_them_back() {
        for (fmt, src) in [
            (Format::Markdown, "hello\n"),
            (Format::Djot, "hello\n"),
            (Format::Html, "<p>hello</p>\n"),
            (Format::Asciidoc, "hello\n"),
        ] {
            // What twig writes for a host's name, read back by the walker.
            let mut ed = twig::Editor::new_ext(src.as_bytes(), fmt, parse_extensions()).unwrap();
            ed.insert_directive(src.len() - 1, "x-card", None, &[("k", Some("v"))])
                .unwrap_or_else(|e| panic!("{fmt:?}: twig spells it: {e}"));
            let written = String::from_utf8(ed.source().unwrap()).unwrap();
            let mut d = fmt_doc(&written, fmt);
            d.view = View::Wysiwyg;
            d.build_visual(80);
            let read_back = d
                .vmap
                .directives
                .iter()
                .any(|m| m.name == "x-card" && m.attr("k") == Some("v"));
            let c = Capabilities::of(fmt);
            assert_eq!(c.directives, read_back, "{fmt:?}: {written:?}");
            assert!(c.page_break, "{fmt:?}");

            // And where it is not offered, it is refused in the format's name.
            if !c.directives {
                let mut d = fmt_doc(src, fmt);
                d.caret = d.source.find("hello").unwrap() + 5;
                d.insert_directive("x-card", None, &[]);
                assert_eq!(d.source, src, "{fmt:?}");
                let status = d.status.as_deref().unwrap_or("");
                assert!(status.contains("not supported"), "{fmt:?}: {status:?}");
            }
        }
        assert!(!Capabilities::of(Format::Xml).directives);
    }

    /// A terminal draws a host's directive as lines of its own and reports how
    /// many, keyed by what the directive says; the placeholder grows to that
    /// height the way a picture does, with blank fillers holding no caret.
    #[test]
    fn set_directive_rows_reserves_filler_rows_for_a_host_drawing() {
        let body = "intro\n\n::x-card[A]{src=\"u\"}\n\n::x-card[B]\n\nend\n";
        let mut d = wysiwyg_doc("directive_rows", body);
        assert_eq!(d.vmap.directives.len(), 2);
        assert!(d.vmap.directives.iter().all(|i| i.rows_span.len() == 1));
        let stops = |d: &mut Doc| {
            d.caret = 0;
            let mut seen = vec![d.caret];
            loop {
                d.move_right(false);
                if *seen.last().unwrap() == d.caret {
                    break;
                }
                seen.push(d.caret);
            }
            seen
        };
        let before = stops(&mut d);

        let key = d.vmap.directives[0].key();
        assert_eq!(
            key,
            wysiwyg::DirectiveKey {
                name: "x-card".into(),
                label: "A".into(),
                attrs: vec![("src".into(), "u".into())],
            }
        );
        d.set_directive_rows(HashMap::from([(key, 4)]));
        d.build_visual(80);
        let span = d.vmap.directives[0].rows_span.clone();
        assert_eq!(span.len(), 4, "the four rows asked for");
        assert_eq!(
            d.vmap.rows[span.start]
                .leaf_directive
                .as_ref()
                .map(|m| m.rows),
            Some(4)
        );
        for r in span.start + 1..span.end {
            let row = &d.vmap.rows[r];
            assert!(row.decoration && row.glyphs.is_empty(), "filler {r}");
            assert!(row.leaf_directive.is_none(), "only the first row is marked");
        }
        assert_eq!(
            d.vmap.directives[1].rows_span.len(),
            1,
            "another label is another key"
        );
        assert_eq!(stops(&mut d), before, "reserving rows adds no stops");

        // Keyed by what it says, not where it stands: an edit above keeps it.
        d.caret = 0;
        d.insert("more ");
        d.build_visual(80);
        assert_eq!(d.vmap.directives[0].rows_span.len(), 4);
    }

    /// The vocabulary's capabilities, per format. The two block properties are
    /// `SetBlockAttrs` and the three run ones `WrapRangeAttrs`, which is why
    /// AsciiDoc can align a paragraph and not size a run: its `[#id.role]#text#`
    /// keeps an id and a role and has no slot for a `data-` key.
    #[test]
    fn the_presentation_capabilities_are_ragged_per_format() {
        for fmt in [Format::Markdown, Format::Djot, Format::Html] {
            let c = Capabilities::of(fmt);
            assert!(c.alignment, "{fmt:?} alignment");
            assert!(c.line_spacing, "{fmt:?} line spacing");
            assert!(c.font_size, "{fmt:?} size");
            assert!(c.font_family, "{fmt:?} face");
            assert!(c.text_color, "{fmt:?} colour");
        }
        // Markdown spells both only under the extensions leaf parses with — a
        // `<div>` and a `<span>` read back as containers under `html_elements`,
        // and `::page-break` as a directive under `directives`. Ask twig's own
        // defaults and the answer is no, which is why `Capabilities` is built
        // with `supports_with`.
        assert!(!Format::Markdown.supports(Gesture::SetBlockAttrs));
        assert!(!Format::Markdown.supports(Gesture::WrapRangeAttrs));
        assert!(!Format::Markdown.supports(Gesture::InsertDirective));

        let adoc = Capabilities::of(Format::Asciidoc);
        assert!(adoc.alignment && adoc.line_spacing, "AsciiDoc's `[…]` line");
        assert!(
            !adoc.font_size && !adoc.font_family && !adoc.text_color,
            "AsciiDoc has no inline spelling that keeps a data- key"
        );

        // XML spells none of it, and neither page break — nor has it blocks a
        // caret could name for a move.
        let xml = Capabilities::of(Format::Xml);
        assert!(!xml.alignment && !xml.font_size && !xml.page_break && !xml.move_block);
        for f in [Format::Markdown, Format::Djot, Format::Html] {
            assert!(Capabilities::of(f).move_block, "{f:?} moves blocks");
        }
        for f in [
            Format::Markdown,
            Format::Djot,
            Format::Html,
            Format::Asciidoc,
        ] {
            assert!(Capabilities::of(f).page_break, "{f:?} breaks pages");
        }
    }

    /// A format that cannot spell a property refuses in its own words and
    /// writes nothing — the guard every other gesture has.
    #[test]
    fn a_presentation_gesture_a_format_cannot_spell_is_refused_with_a_reason() {
        let src = "<doc><p>hello</p></doc>\n";
        #[allow(clippy::type_complexity)]
        let ops: [(&str, &dyn Fn(&mut Doc)); 6] = [
            ("alignment", &|d: &mut Doc| {
                d.set_alignment(Some(Align::Center))
            }),
            ("line spacing", &|d: &mut Doc| {
                d.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)))
            }),
            ("size", &|d: &mut Doc| {
                d.set_font_size(Some(FontSize::Step(SizeStep::Large)))
            }),
            ("face", &|d: &mut Doc| {
                d.set_font_family(Some(FontFace::Generic(FontFamily::Serif)))
            }),
            ("colour", &|d: &mut Doc| {
                d.set_text_color(Some(TextColor::Named(MarkColor::Red)))
            }),
            ("page break", &|d: &mut Doc| d.insert_page_break()),
        ];
        for (name, op) in ops {
            let mut d = fmt_doc(src, Format::Xml);
            let at = d.source.find("hello").unwrap();
            d.caret = at;
            d.anchor = Some(at + 5);
            op(&mut d);
            assert_eq!(d.source, src, "{name} edited an XML document");
            assert!(!d.dirty, "{name} marked the document dirty");
            let status = d.status.as_deref().unwrap_or("");
            assert!(
                status.contains("xml"),
                "{name}: the refusal should name the format, got {status:?}"
            );
        }

        // AsciiDoc is the ragged one: the block gesture works where the run
        // gesture does not, and a *selection* is what tells the two apart.
        let mut adoc = fmt_doc("hello world\n", Format::Asciidoc);
        adoc.anchor = Some(0);
        adoc.caret = 5;
        adoc.set_font_size(Some(FontSize::Step(SizeStep::Large)));
        assert_eq!(adoc.source, "hello world\n", "no inline spelling");
        assert!(adoc.status.is_some());
    }

    /// A read-only document takes none of it, and a caret on a blank line has
    /// no block to carry an attribute — both say so rather than writing.
    #[test]
    fn a_presentation_gesture_respects_read_only_and_a_blank_line() {
        let mut ro = fmt_doc("hello\n", Format::Djot);
        ro.read_only = true;
        ro.caret = 1;
        ro.set_alignment(Some(Align::Center));
        assert_eq!(ro.source, "hello\n");

        let mut blank = fmt_doc("a\n\n\nb\n", Format::Djot);
        blank.caret = 2; // the empty line between the two paragraphs
        blank.set_alignment(Some(Align::Center));
        assert_eq!(blank.source, "a\n\n\nb\n");
        assert!(
            blank.status.as_deref().unwrap_or("").contains("no block"),
            "got {:?}",
            blank.status
        );
    }

    /// A block attribute gesture keeps the caret on the **text** it was on, not
    /// on the byte offset it had. Markdown has nowhere to put a paragraph's
    /// attributes but a `<div>` around it, and twig splices the div and the
    /// block it wraps as one region — so a caret that kept its offset landed in
    /// the markup, and every press after the first answered "no block at the
    /// caret" with the toolbar's queries reading nothing.
    #[test]
    fn a_markdown_block_gesture_keeps_the_caret_on_its_text() {
        let word = |d: &Doc| d.caret - d.source.find("brown").unwrap();
        let mut md = fmt_doc("the quick brown fox\n", Format::Markdown);
        md.caret = md.source.find("brown").unwrap() + 2; // "br|own"

        // Wrapping: the div and two blank lines open above the block.
        md.set_alignment(Some(Align::Center));
        assert_eq!(
            md.source,
            "<div class=\"center\">\n\nthe quick brown fox\n\n</div>\n"
        );
        assert_eq!(word(&md), 2, "the caret left its word: {}", md.caret);
        assert_eq!(md.alignment_at_caret(), Some(Align::Center));

        // Re-styling: the attribute line changes length under the same caret,
        // and the second press reaches the same block rather than nothing.
        md.set_alignment(Some(Align::Right));
        assert_eq!(
            md.source, "<div class=\"right\">\n\nthe quick brown fox\n\n</div>\n",
            "a second press re-styles the div"
        );
        assert_eq!(md.status, None);
        assert_eq!(word(&md), 2);

        // A second key on the same div — the line grows, the caret rides it.
        md.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
        assert_eq!(
            md.source,
            "<div class=\"right\" data-line-height=\"2\">\n\nthe quick brown fox\n\n</div>\n"
        );
        assert_eq!(word(&md), 2);
        assert_eq!(
            md.line_spacing_at_caret(),
            Some(LineHeight::Step(LineSpacing::Double))
        );

        // Unwrapping: the line shrinks, and then the div goes altogether.
        md.set_alignment(None);
        assert_eq!(
            md.source,
            "<div data-line-height=\"2\">\n\nthe quick brown fox\n\n</div>\n"
        );
        assert_eq!(word(&md), 2);
        md.set_line_spacing(None);
        assert_eq!(md.source, "the quick brown fox\n", "the last key unwraps");
        assert_eq!(word(&md), 2, "the caret came back down with the block");
        assert_eq!(md.alignment_at_caret(), None);
        assert_eq!(md.status, None);
    }

    /// The same rule in djot, where the spelling is a `{…}` line *above* the
    /// block rather than a wrapper around it: inserting it pushes the block
    /// down, re-styling it changes the line's length, and clearing the last key
    /// takes the line away again. The caret rides all three.
    #[test]
    fn a_djot_attribute_line_keeps_the_caret_on_its_text() {
        let word = |d: &Doc| d.caret - d.source.find("brown").unwrap();
        let mut dj = fmt_doc("the quick brown fox\n", Format::Djot);
        dj.caret = dj.source.find("brown").unwrap() + 2;

        dj.set_alignment(Some(Align::Center));
        assert_eq!(dj.source, "{.center}\nthe quick brown fox\n");
        assert_eq!(word(&dj), 2);
        assert_eq!(dj.alignment_at_caret(), Some(Align::Center));

        dj.set_line_spacing(Some(LineHeight::Step(LineSpacing::Double)));
        assert_eq!(
            dj.source, "{.center data-line-height=\"2\"}\nthe quick brown fox\n",
            "a second press edits the line the first wrote"
        );
        assert_eq!(word(&dj), 2);

        dj.set_alignment(None);
        assert_eq!(dj.source, "{data-line-height=\"2\"}\nthe quick brown fox\n");
        assert_eq!(word(&dj), 2);

        dj.set_line_spacing(None);
        assert_eq!(dj.source, "the quick brown fox\n");
        assert_eq!(word(&dj), 2);
        assert_eq!(dj.status, None);
    }

    /// The run gestures with no selection are the block gesture, so they keep
    /// the caret the same way — and a heading keeps it inside the heading's own
    /// text, past the `# ` its content span starts after. A selection rides
    /// along whole: a block gesture is not a run gesture, and what was selected
    /// before the press is still selected after it.
    #[test]
    fn a_block_gesture_carries_a_selection_and_a_heading_caret_too() {
        // No selection: the run gesture goes through the block door.
        let mut md = fmt_doc("the quick brown fox\n", Format::Markdown);
        md.caret = md.source.find("brown").unwrap() + 2;
        md.set_font_size(Some(FontSize::Step(SizeStep::Large)));
        assert_eq!(
            md.source,
            "<div data-size=\"large\">\n\nthe quick brown fox\n\n</div>\n"
        );
        assert_eq!(md.caret - md.source.find("brown").unwrap(), 2);
        assert_eq!(
            md.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Large))
        );
        md.set_font_size(Some(FontSize::Step(SizeStep::Small)));
        assert_eq!(
            md.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Small)),
            "the second press reached the same block"
        );

        // A selection: alignment is the block's whatever is selected, and the
        // words stay selected.
        let mut sel = fmt_doc("the quick brown fox\n", Format::Markdown);
        let at = sel.source.find("brown").unwrap();
        sel.anchor = Some(at);
        sel.caret = at + 5;
        sel.set_alignment(Some(Align::Center));
        let now = sel.source.find("brown").unwrap();
        assert_eq!(sel.selection(), Some((now, now + 5)), "the words moved out");

        // A heading: the content span starts past the `# `.
        let mut h = fmt_doc("# hi there\n\nbody\n", Format::Markdown);
        h.caret = h.source.find("there").unwrap() + 1;
        h.set_alignment(Some(Align::Right));
        assert_eq!(
            h.source,
            "<div class=\"right\">\n\n# hi there\n\n</div>\n\nbody\n"
        );
        assert_eq!(h.caret, h.source.find("there").unwrap() + 1);
        assert_eq!(h.alignment_at_caret(), Some(Align::Right));
    }

    /// A djot document open in the rich view, with its map built as
    /// [`wysiwyg_doc`] builds a Markdown one's.
    fn wysiwyg_djot(body: &str) -> Doc {
        let mut d = fmt_doc(body, Format::Djot);
        d.view = View::Wysiwyg;
        d.build_visual(80);
        d
    }

    /// Backspace at the start of a block whose presentation is spelled as
    /// hidden markup before it strips that presentation, the way Backspace at
    /// a heading's start strips its `#`. The ordinary delete fused djot's
    /// `{.center}` line onto the text and took the blank line a Markdown div
    /// needs between its tag and its paragraph.
    #[test]
    fn backspace_at_the_start_of_a_centred_paragraph_strips_its_attributes() {
        let mut md = wysiwyg_doc(
            "wys_attr_bksp",
            "above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n",
        );
        md.caret = md.source.find("hello").unwrap();
        md.backspace();
        assert_eq!(
            md.source, "above\n\nhello\n\nbelow\n",
            "the div is unwrapped"
        );
        assert_eq!(md.caret, 7, "the caret stays at the start of its text");
        md.backspace();
        assert_eq!(
            md.source, "above\nhello\n\nbelow\n",
            "the next press joins the paragraphs, as it always did"
        );

        let mut dj = wysiwyg_djot("above\n\n{.center}\nhello\n\nbelow\n");
        dj.caret = dj.source.find("hello").unwrap();
        dj.backspace();
        assert_eq!(
            dj.source, "above\n\nhello\n\nbelow\n",
            "the attribute line goes"
        );
        assert_eq!(dj.caret, 7);

        // A heading's own marker is the nearer hidden markup, and goes first;
        // the attributes are the next press's.
        let mut dj = wysiwyg_djot("{.center}\n# Title\n");
        dj.caret = dj.source.find("Title").unwrap();
        dj.backspace();
        assert_eq!(dj.source, "{.center}\nTitle\n", "the `#` first");
        dj.build_visual(80);
        dj.backspace();
        assert_eq!(dj.source, "Title\n", "then the attributes");
        assert_eq!(dj.caret, 0);
    }

    /// A div around several blocks has no sole child for twig to unwrap, so
    /// at its first block the caret steps back to the stop before rather than
    /// taking the div apart; a later block has an ordinary paragraph above it
    /// and joins as any paragraph does.
    #[test]
    fn backspace_at_the_first_of_a_div_s_blocks_steps_back_and_a_later_one_joins() {
        let src = "above\n\n<div class=\"center\">\n\nhello\n\nworld\n\n</div>\n";
        let mut d = wysiwyg_doc("wys_div_first", src);
        d.caret = d.source.find("hello").unwrap();
        d.backspace();
        assert_eq!(d.source, src, "nothing is deleted");
        assert_eq!(d.caret, 5, "the caret steps back to the end of `above`");

        let mut d = wysiwyg_doc("wys_div_later", src);
        d.caret = d.source.find("world").unwrap();
        d.backspace();
        assert_eq!(
            d.source, "above\n\n<div class=\"center\">\n\nhello\nworld\n\n</div>\n",
            "a later block joins the one above it"
        );
    }

    /// Backspace at the start of the paragraph after a Markdown div joins it
    /// into the div's last paragraph — the join any two paragraphs make, with
    /// the hidden `</div>` carried past the joined text. The ordinary delete
    /// took the newline under the tag, which drew nothing different, and the
    /// next press took the `>` and left the div unclosed.
    #[test]
    fn backspace_after_a_div_joins_the_paragraph_into_it() {
        let src = "above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n";
        let mut d = wysiwyg_doc("wys_div_join", src);
        d.caret = d.source.find("below").unwrap();
        d.backspace();
        assert_eq!(
            d.source,
            "above\n\n<div class=\"center\">\n\nhello\nbelow\n\n</div>\n"
        );
        assert_eq!(
            d.caret,
            d.source.find("below").unwrap(),
            "the caret stays at the start of the joined text"
        );
        d.build_visual(80);
        assert_eq!(
            d.alignment_at_caret(),
            Some(Align::Center),
            "and is centred now"
        );
        d.undo();
        assert_eq!(d.source, src, "one undo step");

        // A list closes the div: the paragraph joins the last item's text,
        // under the item's continuation indent, inside the div.
        let src = "<div class=\"center\">\n\n- item\n\n</div>\n\nbelow\n";
        let mut d = wysiwyg_doc("wys_div_list", src);
        d.caret = d.source.find("below").unwrap();
        d.backspace();
        assert_eq!(
            d.source, "<div class=\"center\">\n\n- item\n  below\n\n</div>\n",
            "the paragraph joins the item"
        );
        assert_eq!(d.caret, d.source.find("below").unwrap());

        // And where twig has nothing to join into — a code block above — the
        // caret steps back to the stop before, and nothing is deleted.
        let src = "```\ncode\n```\n\nbelow\n";
        let mut d = wysiwyg_doc("wys_code_then_para", src);
        d.caret = d.source.find("below").unwrap();
        d.backspace();
        assert_eq!(d.source, src, "nothing is deleted");
        assert!(
            d.caret < d.source.find("below").unwrap(),
            "the caret stepped back"
        );
    }

    /// Backspace on a blank line collapses to the stop before it — but not
    /// across hidden markup, which that collapse deleted whole: a `</div>`,
    /// or a comment between two blocks. There the blank line goes alone, and
    /// the caret lands where the collapse would have put it.
    #[test]
    fn backspace_on_a_blank_line_after_hidden_markup_keeps_the_markup() {
        let mut d = wysiwyg_doc(
            "wys_div_blank",
            "<div class=\"center\">\n\nhello\n\n</div>\n\n\n\nbelow\n",
        );
        d.caret = d.source.find("below").unwrap() - 2; // the empty paragraph
        assert!(
            d.vmap.is_stop(d.caret),
            "the empty paragraph is a caret home"
        );
        d.backspace();
        assert_eq!(
            d.source, "<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n",
            "the blank line goes and the div stays closed"
        );
        assert_eq!(
            d.caret,
            d.source.find("hello").unwrap() + 5,
            "onto the end of `hello`"
        );

        let mut d = wysiwyg_doc("wys_comment_blank", "above\n\n<!-- note -->\n\n\n\nbelow\n");
        d.caret = d.source.find("below").unwrap() - 2;
        assert!(d.vmap.is_stop(d.caret));
        d.backspace();
        assert_eq!(
            d.source, "above\n\n<!-- note -->\n\nbelow\n",
            "the comment stays"
        );
        assert_eq!(d.caret, 5);
    }

    /// Backspace at the end of an attributed span steps inside its hidden
    /// closing tag the way it steps inside a `**`, and takes the span with
    /// its last letter. Before, the byte-step took the `>` of `</span>`,
    /// which left the paragraph unparseable: it vanished from the rich view,
    /// and the Backspace after that joined the next block into the wreck.
    #[test]
    fn backspace_walks_into_a_sized_span_and_takes_the_emptied_span_with_its_space() {
        let src = "above\n\nThis <span data-size=\"x-large\">is</span> a test\n\nTest 2\n";
        let mut d = wysiwyg_doc("wys_span_bs", src);
        d.caret = d.source.find("a test").unwrap() + 6;
        for _ in 0..7 {
            d.backspace();
        }
        assert_eq!(
            d.source,
            "above\n\nThis <span data-size=\"x-large\">is</span>\n\nTest 2\n"
        );
        d.backspace();
        assert_eq!(
            d.source, "above\n\nThis <span data-size=\"x-large\">i</span>\n\nTest 2\n",
            "the first Backspace after the tag takes the letter, not the `>`"
        );
        d.backspace();
        assert_eq!(
            d.source, "above\n\nThis \n\nTest 2\n",
            "the last letter takes the span with it"
        );
        assert_eq!(d.caret, 12, "the caret is where the letter was");
        d.backspace();
        assert_eq!(d.source, "above\n\nThis\n\nTest 2\n");
        assert_eq!(d.caret, 11);
        d.build_visual(80);
        assert!(
            d.vmap
                .rows
                .iter()
                .any(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>() == "This"),
            "the paragraph is still drawn"
        );
    }

    #[test]
    fn backspace_walks_into_a_djot_sized_span_too() {
        let mut d = wysiwyg_djot("This [is]{data-size=\"x-large\"}\n\nTest 2\n");
        d.caret = d.source.find("\n\nTest 2").unwrap();
        // The caret home at the paragraph's end is inside the span, before
        // its `]`: the map offers no stop after `]{…}`.
        d.build_visual(80);
        assert_eq!(d.vmap.stop_before(31), Some(8));
        d.caret = 8;
        d.backspace();
        assert_eq!(d.source, "This [i]{data-size=\"x-large\"}\n\nTest 2\n");
        d.backspace();
        assert_eq!(
            d.source, "This \n\nTest 2\n",
            "the attribute block outside the span goes with it"
        );
        d.backspace();
        assert_eq!(d.source, "This\n\nTest 2\n");
        assert_eq!(d.caret, 4);
    }

    /// A span that is empty as the file was written has no stop of its own;
    /// Backspace reaching it from behind takes it with the character before
    /// it, the character the key looked aimed at.
    #[test]
    fn backspace_over_an_already_empty_span_takes_it_with_the_character_before() {
        let src = "This <span data-size=\"x-large\"></span> a test\n";
        let mut d = wysiwyg_doc("wys_span_empty", src);
        d.caret = d.source.find(" a test").unwrap();
        d.backspace();
        assert_eq!(d.source, "This a test\n");
        assert_eq!(d.caret, 4);
    }

    /// The mirror: Delete in front of a span's opening tag takes its first
    /// letter, and the span with its last.
    #[test]
    fn delete_walks_into_a_sized_span_and_takes_the_span_with_its_last_letter() {
        let src = "This <span data-size=\"x-large\">is</span> a test\n";
        let mut d = wysiwyg_doc("wys_span_del", src);
        d.caret = 5;
        d.delete_forward();
        assert_eq!(
            d.source,
            "This <span data-size=\"x-large\">s</span> a test\n"
        );
        d.delete_forward();
        assert_eq!(d.source, "This  a test\n");
        assert_eq!(d.caret, 5);
        d.delete_forward();
        assert_eq!(d.source, "This a test\n");
        assert_eq!(d.caret, 5);
    }

    /// The block version of the span's emptying rule. Centre a one-letter
    /// paragraph — Markdown spells that as a `<div>` around it — and
    /// Backspace the letter: the div goes with it, leaving a plain blank line
    /// the caret is at home on, in the incremental map and the from-scratch
    /// one alike. Before, the letter went alone; the emptied div drew a
    /// caret home only the stale map had, and the next Backspace collapsed
    /// the line and left `<div class="center">\n\n</div>` standing invisibly
    /// in the file.
    #[test]
    fn backspace_that_empties_a_centred_paragraph_takes_its_div_with_the_letter() {
        let mut d = wysiwyg_doc("wys_div_empty", "Try the toolbar.\n\nT\n");
        d.caret = d.source.len() - 1;
        d.set_alignment(Some(Align::Center));
        assert_eq!(
            d.source,
            "Try the toolbar.\n\n<div class=\"center\">\n\nT\n\n</div>\n"
        );
        d.backspace();
        assert_eq!(d.source, "Try the toolbar.\n\n\n");
        assert_eq!(d.caret, 18, "on the blank line where the letter was");
        // The host rebuilds the map after every key; the spliced map and a
        // fresh one both give the line a caret home.
        d.build_visual_unwrapped();
        assert!(d.vmap.is_stop(18));
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after the div goes");
        d.backspace();
        assert_eq!(
            d.source, "Try the toolbar.\n",
            "then the blank line collapses"
        );
        assert_eq!(d.caret, 16);

        // With a block after the div the blank line keeps a gap each side.
        let mut d = wysiwyg_doc(
            "wys_div_empty_mid",
            "Try the toolbar.\n\n<div class=\"center\">\n\nT\n\n</div>\n\nbelow\n",
        );
        d.caret = d.source.find("T\n").unwrap() + 1;
        d.backspace();
        assert_eq!(d.source, "Try the toolbar.\n\n\n\nbelow\n");
        assert_eq!(d.caret, 18);
        d.build_visual_unwrapped();
        assert!(d.vmap.is_stop(18));
        wysiwyg::assert_maps_eq(&d.vmap, &reference_map(&d.source), "after the div goes");
    }

    /// djot spells the same paragraph as a `{.center}` line above it, and
    /// twig has no node at all for that line once the paragraph is gone —
    /// so the line goes with the letter too.
    #[test]
    fn backspace_that_empties_a_centred_paragraph_takes_its_djot_attrs_line_too() {
        let mut d = fmt_doc("Try the toolbar.\n\nT\n", Format::Djot);
        d.build_visual(80);
        d.caret = d.source.len() - 1;
        d.set_alignment(Some(Align::Center));
        assert_eq!(d.source, "Try the toolbar.\n\n{.center}\nT\n");
        d.backspace();
        assert_eq!(d.source, "Try the toolbar.\n\n\n");
        assert_eq!(d.caret, 18);
        d.build_visual(80);
        assert!(d.vmap.is_stop(18));
    }

    /// The rule is for a block that would be no block: a div holding more
    /// keeps its tags, and an emptied heading is still a heading.
    #[test]
    fn emptying_a_paragraph_keeps_a_div_that_holds_more_and_a_heading_its_marker() {
        let mut d = wysiwyg_doc(
            "wys_div_more",
            "<div class=\"center\">\n\nText\n\nT\n\n</div>\n",
        );
        d.caret = d.source.find("T\n").unwrap() + 1;
        d.backspace();
        assert_eq!(d.source, "<div class=\"center\">\n\nText\n\n\n\n</div>\n");
        assert_eq!(d.caret, 28, "the blank line inside the div, as after Enter");

        let mut d = wysiwyg_doc(
            "wys_div_heading",
            "<div class=\"center\">\n\n# T\n\n</div>\n",
        );
        d.caret = d.source.find("T\n").unwrap() + 1;
        d.backspace();
        assert_eq!(d.source, "<div class=\"center\">\n\n# \n\n</div>\n");
        assert_eq!(d.caret, 24);
        d.build_visual(80);
        assert!(
            d.vmap.is_stop(24),
            "the empty heading is still a caret home"
        );
    }

    /// The mirror: Delete in front of the letter takes the div with it.
    #[test]
    fn delete_that_empties_a_centred_paragraph_takes_its_div_with_the_letter() {
        let mut d = wysiwyg_doc(
            "wys_div_empty_del",
            "Try the toolbar.\n\n<div class=\"center\">\n\nT\n\n</div>\n",
        );
        d.caret = d.source.find("T\n").unwrap();
        d.delete_forward();
        assert_eq!(d.source, "Try the toolbar.\n\n\n");
        assert_eq!(d.caret, 18);
        d.build_visual(80);
        assert!(d.vmap.is_stop(18));

        let mut d = fmt_doc("Try the toolbar.\n\n{.center}\nT\n", Format::Djot);
        d.build_visual(80);
        d.caret = d.source.find("T\n").unwrap();
        d.delete_forward();
        assert_eq!(d.source, "Try the toolbar.\n\n\n");
        assert_eq!(d.caret, 18);
    }

    /// Backspace at a block's start is twig's join, spelled per format — so
    /// the cases the one-newline delete got wrong come out right: a
    /// paragraph joins onto a heading's line, HTML's `</p><p>` goes as one,
    /// and a quote's prefix is written on the joined line.
    #[test]
    fn backspace_at_a_block_start_joins_it_the_format_s_way() {
        let mut d = wysiwyg_doc("wys_join_heading", "# Title\n\nbelow\n");
        d.caret = d.source.find("below").unwrap();
        d.backspace();
        assert_eq!(d.source, "# Title below\n", "onto the heading's line");
        assert_eq!(d.caret, d.source.find("below").unwrap());
        d.undo();
        assert_eq!(d.source, "# Title\n\nbelow\n", "one undo step");

        let mut d = wysiwyg_doc("wys_join_quote", "> a\n\nb\n");
        d.caret = d.source.find('b').unwrap();
        d.backspace();
        assert_eq!(d.source, "> a\n> b\n", "into the quote, with its prefix");
        assert_eq!(d.caret, d.source.find('b').unwrap());

        let mut h = fmt_doc("<p>above</p>\n<p class=\"x\">below</p>\n", Format::Html);
        h.view = View::Wysiwyg;
        h.build_visual(80);
        h.caret = h.source.find("below").unwrap();
        h.backspace();
        assert_eq!(
            h.source, "<p>above\nbelow</p>\n",
            "one paragraph, the tag gone whole"
        );
        assert_eq!(h.caret, h.source.find("below").unwrap());
    }

    /// A selection from the end of a run's text to the next block's start is
    /// the run's closing delimiters and a blank line in the source, and one
    /// `"\n"` on screen: deleting it — iOS's `replace(_:withText:)` one stop
    /// back — joins the blocks and keeps the run closed. It took the `***`
    /// with the newlines and left the opening ones on screen.
    #[test]
    fn deleting_a_selection_keeps_the_delimiters_of_a_run_it_cuts() {
        let src = "good.\n\n***HENRY B EYRING***\n\nWhy is life\n";
        let mut d = wysiwyg_doc("sel_cut_close", src);
        let end = d.source.find("G***").unwrap() + 1;
        d.place_caret(end, false);
        d.place_caret(d.source.find("Why").unwrap(), true);
        d.insert("");
        assert_eq!(d.source, "good.\n\n***HENRY B EYRING***Why is life\n");
        assert_eq!(d.caret, end, "at the end of the run's text");

        // On one line, from inside a run to past it, and typing over it.
        let mut d = wysiwyg_doc("sel_cut_line", "a **bold** text\n");
        d.place_caret(d.source.find("ld").unwrap(), false);
        d.place_caret(d.source.find("xt").unwrap(), true);
        d.backspace();
        assert_eq!(d.source, "a **bo**xt\n");
        let mut d = wysiwyg_doc("sel_cut_type", "a **bold** text\n");
        d.place_caret(d.source.find("ld").unwrap(), false);
        d.place_caret(d.source.find("xt").unwrap(), true);
        d.insert("X");
        assert_eq!(d.source, "a **boX**xt\n", "the text lands in the run");

        // Cut at its start, the run keeps its opening delimiters, and the
        // text typed goes in front of them.
        let mut d = wysiwyg_doc("sel_cut_open", "plain **bold**\n");
        d.place_caret(3, false);
        d.place_caret(d.source.find("ld").unwrap(), true);
        d.insert("X");
        assert_eq!(d.source, "plaX**ld**\n");

        // A run whose whole text goes, with more past it, goes whole.
        let mut d = wysiwyg_doc("sel_cut_whole", "a **bold** text\n");
        d.place_caret(d.source.find("bold").unwrap(), false);
        d.place_caret(d.source.find("xt").unwrap(), true);
        d.backspace();
        assert_eq!(d.source, "a xt\n");
    }

    /// A paste of several paragraphs over a selection that cuts runs: the run
    /// cut at its end closes after the paste's first line, and the run cut
    /// at its start opens after its last, so each stays on one paragraph.
    #[test]
    fn a_paste_of_paragraphs_over_a_cut_run_closes_it_on_its_own_line() {
        let src = "a **bold** and *italic* text\n";
        let mut d = wysiwyg_doc("sel_cut_paste", src);
        d.place_caret(d.source.find("ld").unwrap(), false);
        d.place_caret(d.source.find("alic").unwrap(), true);
        d.paste("one\n\ntwo");
        assert_eq!(d.source, "a **boone**\n\ntwo*alic* text\n");
        assert_eq!(d.caret, d.source.find("*alic").unwrap(), "after the paste");

        // A paste ending in a space puts the space outside the run, where a
        // closing delimiter can still close it.
        let mut d = wysiwyg_doc("sel_cut_paste_space", "a **bold** text\n");
        d.place_caret(d.source.find("ld").unwrap(), false);
        d.place_caret(d.source.find("xt").unwrap(), true);
        d.paste("X ");
        assert_eq!(d.source, "a **boX** xt\n");
    }

    /// Enter over a selection that cuts a run deletes it and splits there,
    /// so the run closes on the line it is on.
    #[test]
    fn enter_over_a_selection_that_cuts_a_run_splits_where_it_was() {
        let mut d = wysiwyg_doc("sel_cut_enter", "a **bold** text\n");
        d.place_caret(d.source.find("ld").unwrap(), false);
        d.place_caret(d.source.find("xt").unwrap(), true);
        d.newline();
        assert_eq!(d.source, "a **bo**\n\nxt\n");
    }

    /// Delete at the end of a block's content is the same join aimed at the
    /// block after it, and the caret stays where the joined text now begins.
    #[test]
    fn delete_at_a_block_end_joins_the_next_block_into_it() {
        let src = "above\n\n<div class=\"center\">\n\nhello\n\n</div>\n\nbelow\n";
        let mut d = wysiwyg_doc("wys_del_join", src);
        d.caret = d.source.find("hello").unwrap() + 5;
        d.delete_forward();
        assert_eq!(
            d.source, "above\n\n<div class=\"center\">\n\nhello\nbelow\n\n</div>\n",
            "below joins hello inside the div"
        );
        assert_eq!(
            d.caret,
            d.source.find("hello").unwrap() + 5,
            "the caret stays"
        );

        let mut d = wysiwyg_doc("wys_del_join_head", "above\n\n# Title\n");
        d.caret = 5;
        d.delete_forward();
        assert_eq!(
            d.source, "above\nTitle\n",
            "the heading's marker goes with the join"
        );
        assert_eq!(d.caret, 5);

        // A code block after the paragraph: nothing to join, the caret steps
        // forward onto the next stop and nothing is deleted.
        let src = "above\n\n```\ncode\n```\n";
        let mut d = wysiwyg_doc("wys_del_code", src);
        d.caret = 5;
        d.delete_forward();
        assert_eq!(d.source, src);
        assert!(d.caret > 5, "the caret stepped forward");
    }

    // ── Text statistics ──────────────────────────────────────────────────────

    /// The counts of `body`, from a document open in the WYSIWYG view — the
    /// shape every case below starts from.
    fn counts_of(name: &str, body: &str) -> TextCounts {
        doc_in(View::Wysiwyg, name, body).counts()
    }

    #[test]
    fn counts_tally_plain_prose() {
        let c = counts_of(
            "counts_prose",
            "The quick brown fox jumps over the lazy dog.\n",
        );
        assert_eq!(
            c,
            TextCounts {
                words: 9,
                characters: 44,
                characters_without_spaces: 36,
                paragraphs: 1,
            }
        );
    }

    #[test]
    fn counts_read_the_text_and_not_the_markup() {
        // The `**` are four bytes of source and no part of the word.
        assert_eq!(
            counts_of("counts_marks", "a **bold** word\n"),
            TextCounts {
                words: 3,
                characters: 11,
                characters_without_spaces: 9,
                paragraphs: 1,
            }
        );
        // A link is its label; the destination is plumbing, however long.
        assert_eq!(
            counts_of(
                "counts_link",
                "see [the label](https://example.com/a/b/c) here\n"
            ),
            TextCounts {
                words: 4,
                characters: 18,
                characters_without_spaces: 15,
                paragraphs: 1,
            }
        );
    }

    #[test]
    fn counts_spend_nothing_on_a_picture() {
        // A block image renders as a `🖼 alt` placeholder — a picture, not a
        // sentence, and not a paragraph either.
        assert_eq!(
            counts_of("counts_image", "![a long caption](pic.png)\n"),
            TextCounts::default()
        );
        // And it adds nothing to the prose around it.
        assert_eq!(
            counts_of("counts_image_prose", "text\n\n![a long caption](pic.png)\n"),
            TextCounts {
                words: 1,
                characters: 4,
                characters_without_spaces: 4,
                paragraphs: 1,
            }
        );
    }

    #[test]
    fn counts_spend_nothing_on_drawn_furniture() {
        // A thematic break is drawn, not written, and an empty paragraph has
        // nothing in it — neither is a paragraph of the document.
        assert_eq!(
            counts_of("counts_rule", "one\n\n---\n\ntwo\n"),
            TextCounts {
                words: 2,
                characters: 6,
                characters_without_spaces: 6,
                paragraphs: 2,
            }
        );
    }

    #[test]
    fn counts_measure_characters_as_a_reader_does() {
        // Four Han characters (each its own word under UAX#29), one ZWJ emoji
        // family that is a single grapheme cluster, and two letters.
        let c = counts_of(
            "counts_graphemes",
            "你好世界 👩\u{200d}👩\u{200d}👧\u{200d}👦 ok\n",
        );
        assert_eq!(
            c,
            TextCounts {
                words: 5,
                characters: 9,
                characters_without_spaces: 7,
                paragraphs: 1,
            }
        );
    }

    #[test]
    fn counts_take_a_code_block_as_one_paragraph() {
        let c = counts_of("counts_code", "```rust\nlet x = 1;\n\nlet y = 2;\n```\n");
        assert_eq!(
            c,
            TextCounts {
                words: 6,
                characters: 20,
                characters_without_spaces: 14,
                paragraphs: 1,
            }
        );
    }

    #[test]
    fn counts_take_a_table_as_one_paragraph() {
        // The box-drawn borders and the column padding are the renderer's, not
        // the author's; the cells are what was written.
        let c = counts_of("counts_table", "| a b | c |\n| - | - |\n| d | e |\n");
        assert_eq!(
            c,
            TextCounts {
                words: 5,
                characters: 6,
                characters_without_spaces: 5,
                paragraphs: 1,
            }
        );
    }

    #[test]
    fn counts_give_every_item_and_every_quoted_paragraph_its_own_paragraph() {
        let c = counts_of(
            "counts_blocks",
            "- one\n- two\n- three\n\n> first quoted\n>\n> second quoted\n",
        );
        assert_eq!(
            c,
            TextCounts {
                words: 7,
                characters: 36,
                characters_without_spaces: 34,
                paragraphs: 5,
            }
        );
    }

    #[test]
    fn counts_leave_the_frontmatter_out() {
        // The WYSIWYG view doesn't render it and a writer didn't write it.
        let c = counts_of(
            "counts_frontmatter",
            "---\ntitle: Hidden\n---\n\nvisible words here\n",
        );
        assert_eq!(
            c,
            TextCounts {
                words: 3,
                characters: 18,
                characters_without_spaces: 16,
                paragraphs: 1,
            }
        );
    }

    #[test]
    fn counts_of_an_empty_document_are_all_zero() {
        assert_eq!(counts_of("counts_empty", ""), TextCounts::default());
    }

    /// UAX#29 puts a boundary at the hyphen, so a hyphenated compound is two
    /// words. Recorded rather than corrected: it is what the algorithm says,
    /// and what every other UAX#29 counter reports.
    #[test]
    fn counts_split_a_hyphenated_compound_in_two() {
        let c = counts_of("counts_hyphen", "well-known example\n");
        assert_eq!(c.words, 3);
        assert_eq!(c.characters, 18);
        // Punctuation on its own is no word, and an apostrophe doesn't split one.
        assert_eq!(counts_of("counts_punct", "don't ... stop\n").words, 2);
    }

    #[test]
    fn selection_counts_measure_the_selection_and_nothing_without_one() {
        let src = "alpha beta\n\ngamma delta\n";
        let mut d = doc_in(View::Wysiwyg, "counts_sel", src);
        assert_eq!(d.selection_counts(), None, "no selection, no counts");

        // From the `b` of `beta` to the end of `gamma`: two blocks clipped.
        d.select_range(6, 17);
        assert_eq!(
            d.selection_counts(),
            Some(TextCounts {
                words: 2,
                characters: 9,
                characters_without_spaces: 9,
                paragraphs: 2,
            })
        );
    }

    /// The count is of the document, not of the window it is shown in — so
    /// ⌘E must not move it, and neither must a resize or an edit made with no
    /// map built at all.
    #[test]
    fn counts_agree_across_the_views() {
        let src =
            "# Head\n\nA **bold** word, a [label](http://x), and more.\n\n- item one\n- item two\n";
        let mut d = doc_in(View::Wysiwyg, "counts_views", src);
        let wysiwyg = d.counts();
        assert!(wysiwyg.words > 0 && wysiwyg.paragraphs == 4);

        d.toggle_view();
        assert_eq!(d.view, View::Source);
        d.build_source();
        assert_eq!(d.counts(), wysiwyg, "the source view counts the same text");

        // A narrower measure is a narrower window, not a shorter document.
        d.toggle_view();
        d.build_visual(24);
        assert_eq!(d.counts(), wysiwyg, "wrapping is not an input");

        // And an edit made in the source view, with the visual map left stale,
        // still counts the document as it now stands.
        d.toggle_view();
        d.caret = d.source.len();
        d.insert("\n\ntail words\n");
        let after = d.counts();
        assert_eq!(after.paragraphs, wysiwyg.paragraphs + 1);
        assert_eq!(after.words, wysiwyg.words + 2);
    }

    // ── math ─────────────────────────────────────────────────────────────────

    #[test]
    fn a_formula_reveals_on_the_caret_line_in_the_hidden_modes() {
        // The rule the proposal states: a formula's content is its TeX, not
        // its picture, so it reveals on the caret's line in *every* mode —
        // and nothing else on that line does outside `Full`.
        let mut d = doc_in(
            View::Wysiwyg,
            "math_reveal",
            "*one* $x+y$ here\n\ntwo there\n",
        );
        d.set_inline_pictures(true);
        assert_eq!(d.markup_mode(), MarkupMode::None);

        // Away from the formula's line: the atom, and no reveal at all.
        caret_at(&mut d, "two");
        assert!(
            drawn_rows(&d).iter().any(|r| r == "one ∑ here"),
            "{:?}",
            drawn_rows(&d)
        );
        assert_eq!(d.vmap.math.len(), 1);
        assert_eq!(d.reveal_line(), None, "a line with no math keys nothing");

        // On it: the formula is its source, the emphasis is still resolved.
        caret_at(&mut d, "here");
        assert!(
            drawn_rows(&d).iter().any(|r| r == "one $x+y$ here"),
            "{:?}",
            drawn_rows(&d)
        );
        assert!(d.vmap.math.is_empty());
        assert_eq!(d.reveal_line(), Some(Reveal::math(0..16)));

        // Off again, and the picture is back.
        caret_at(&mut d, "two");
        assert!(drawn_rows(&d).iter().any(|r| r == "one ∑ here"));

        // The same in Shortcuts; and Full reveals the emphasis too.
        d.set_markup_mode(MarkupMode::Shortcuts);
        caret_at(&mut d, "here");
        assert!(drawn_rows(&d).iter().any(|r| r == "one $x+y$ here"));
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "here");
        assert!(drawn_rows(&d).iter().any(|r| r == "*one* $x+y$ here"));
    }

    #[test]
    fn an_unrevealed_map_shows_the_carets_line_as_a_page_does() {
        let mut d = doc_in(
            View::Wysiwyg,
            "math_unrevealed",
            "*one* $x+y$ here\n\ntwo there\n",
        );
        d.set_inline_pictures(true);
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "here");
        assert!(drawn_rows(&d).iter().any(|r| r == "*one* $x+y$ here"));
        let (screen, caret) = (d.visual_key(), d.caret);

        // On paper: the delimiters hidden and the formula a picture, though
        // the caret has not moved off the line.
        d.set_unrevealed(true);
        d.build_visual(80);
        assert!(
            drawn_rows(&d).iter().any(|r| r == "one ∑ here"),
            "{:?}",
            drawn_rows(&d)
        );
        assert_eq!(d.vmap.math.len(), 1);
        assert_eq!(d.caret, caret);

        // Off again: the screen's map is back as it was, and the next build
        // reuses it rather than building it over.
        d.set_unrevealed(false);
        assert_eq!(d.visual_key(), screen);
        d.build_visual(80);
        assert_eq!(d.visual_key(), screen);
        assert!(drawn_rows(&d).iter().any(|r| r == "*one* $x+y$ here"));
        assert_eq!(d.caret, caret);
    }

    #[test]
    fn a_formula_closed_by_typing_reveals_at_once() {
        // The reveal line is decided from the last build's layout, which
        // across an edit is stale: the keystroke that closes a `$…$` asks a
        // layout that knew no math. `build_map` asks again once the new
        // layout is in, so the formula does not snap to its picture under
        // the caret.
        let mut d = doc_in(View::Wysiwyg, "math_typed", "say \n");
        d.set_inline_pictures(true);
        d.set_markup_mode(MarkupMode::Shortcuts);
        d.caret = 4;
        for ch in ["$", "x", "$"] {
            d.insert(ch);
            d.build_visual(80);
        }
        assert_eq!(d.source, "say $x$\n");
        assert_eq!(
            drawn_rows(&d)[0],
            "say $x$",
            "source, not a picture, under the caret"
        );
        assert!(d.vmap.math.is_empty());
        // Leaving the line folds it — there is only one line, so add one.
        d.newline();
        d.insert("more");
        d.build_visual(80);
        assert_eq!(drawn_rows(&d)[0], "say ∑");
        assert_eq!(d.vmap.math.len(), 1);
        // And deleting the formula while revealed drops the reveal with it.
        d.caret = 7;
        d.build_visual(80);
        assert_eq!(drawn_rows(&d)[0], "say $x$");
        for _ in 0..3 {
            d.backspace();
        }
        d.build_visual(80);
        assert_eq!(d.source, "say \n\nmore\n");
        assert_eq!(d.reveal_line(), None);
    }

    #[test]
    fn a_display_block_is_edited_where_it_stands() {
        let mut d = doc_in(
            View::Wysiwyg,
            "math_block",
            "intro\n\n$$\n\\int_0^1 x\n$$\n\nend\n",
        );
        caret_at(&mut d, "end");
        assert_eq!(
            drawn_rows(&d),
            vec!["intro", "", "∑ \\int_0^1 x", "", "end"]
        );
        // Up from `end` lands on the placeholder, whose glyphs all carry the
        // block's start — which is on its `$$` line, so the block reveals.
        d.move_up(false);
        d.build_visual(80);
        assert_eq!(d.caret, 7);
        assert_eq!(
            drawn_rows(&d),
            vec!["intro", "", "$$", "\\int_0^1 x", "$$", "", "end"]
        );
        // Down walks the source lines, still revealed; typing edits the TeX.
        d.move_down(false);
        d.build_visual(80);
        assert_eq!(d.caret, 10);
        d.move_end(false);
        d.insert("^2");
        d.build_visual(80);
        assert_eq!(d.source, "intro\n\n$$\n\\int_0^1 x^2\n$$\n\nend\n");
        assert_eq!(drawn_rows(&d)[3], "\\int_0^1 x^2");
        // Out below, and it folds to the placeholder with the new TeX.
        d.move_down(false);
        d.move_down(false);
        d.move_down(false);
        d.build_visual(80);
        assert_eq!(drawn_rows(&d)[2], "∑ \\int_0^1 x^2");
        assert_eq!(d.vmap.math[0].tex, "\n\\int_0^1 x^2\n");
    }

    #[test]
    fn a_frontend_that_edits_math_keeps_the_picture_on_the_caret_line() {
        let src = "*one* $x+y$ here\n\n$$\n\\int_0^1 x\n$$\n\nend\n";
        let mut d = doc_in(View::Wysiwyg, "math_edits", src);
        d.set_inline_pictures(true);
        d.set_owned_regions(&["math"]);
        // On the formula's line, and on the block's: still pictures.
        caret_at(&mut d, "here");
        assert!(
            drawn_rows(&d).iter().any(|r| r == "one ∑ here"),
            "{:?}",
            drawn_rows(&d)
        );
        assert_eq!(d.reveal_line(), None);
        d.move_down(false);
        d.move_down(false);
        d.build_visual(80);
        assert_eq!(d.vmap.math.len(), 2, "{:?}", drawn_rows(&d));
        // Each formula is a region that says where it is and where its TeX
        // is, exactly.
        assert_eq!(d.vmap.regions.len(), 2);
        for (r, m) in d.vmap.regions.iter().zip(&d.vmap.math) {
            assert_eq!(r.kind, "math");
            assert_eq!(r.src, m.src);
            assert_eq!(r.inline, m.glyph.is_some());
            assert_eq!(&d.source[r.content.clone()], m.tex);
            assert!(d.source[r.src..r.end].starts_with('$'));
            assert!(d.source[r.src..r.end].ends_with('$'));
        }
        assert_eq!(&d.source[d.vmap.regions[0].content.clone()], "x+y");
        // An edit above shifts the reused block; its spans move with it.
        d.place_caret(0, false);
        d.insert("Z");
        d.build_visual(80);
        for (r, m) in d.vmap.regions.iter().zip(&d.vmap.math) {
            assert_eq!(&d.source[r.content.clone()], m.tex);
        }
        // Full still reveals everything.
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "here");
        assert!(drawn_rows(&d).iter().any(|r| r.contains("$x+y$")));
    }

    #[test]
    fn a_formula_nobody_owns_is_no_region() {
        let mut d = doc_in(View::Wysiwyg, "math_unowned", "say $x$ here\n\nnext\n");
        d.set_inline_pictures(true);
        d.place_caret(d.source.len(), false);
        d.build_visual(80);
        assert_eq!(d.vmap.math.len(), 1);
        assert!(d.vmap.regions.is_empty());
        d.set_owned_regions(&["fence:mermaid"]);
        d.build_visual(80);
        assert!(
            d.vmap.regions.is_empty(),
            "owning fences owned formulas too"
        );
    }

    #[test]
    fn an_owned_fence_is_one_placeholder_row_and_a_region() {
        let src = "intro\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n```rust\nfn x() {}\n```\n\nend\n";
        let mut d = doc_in(View::Wysiwyg, "fence_owned", src);
        d.set_owned_regions(&["fence:mermaid"]);
        caret_at(&mut d, "end");
        let rows = drawn_rows(&d);
        assert!(rows.iter().any(|r| r == "▤ mermaid"), "{rows:?}");
        assert!(!rows.iter().any(|r| r.contains("A-->B")), "{rows:?}");
        // The other language is still code.
        assert!(rows.iter().any(|r| r == "fn x() {}"), "{rows:?}");
        assert_eq!(d.vmap.regions.len(), 1);
        let r = d.vmap.regions[0].clone();
        assert_eq!(r.kind, "fence:mermaid");
        assert!(!r.inline);
        assert_eq!(&d.source[r.content.clone()], "graph TD\n  A-->B\n");
        assert!(d.source[r.src..r.end].starts_with("```mermaid"));
        assert!(d.source[r.src..r.end].trim_end().ends_with("```"));
        // The caret rests either side of it, and on its line it stays a
        // placeholder.
        d.place_caret(r.src, false);
        d.build_visual(80);
        assert!(drawn_rows(&d).iter().any(|row| row == "▤ mermaid"));
        assert_eq!(d.caret, r.src);
        d.move_right(false);
        assert_eq!(d.caret, r.end, "one step crosses the block");
        // An edit through its content moves the end and keeps the region.
        d.edit(r.content.start, r.content.start, "%% note\n");
        d.build_visual(80);
        let r2 = d.vmap.regions[0].clone();
        assert_eq!(
            &d.source[r2.content.clone()],
            "%% note\ngraph TD\n  A-->B\n"
        );
        assert_eq!(r2.end, r.end + 8);
        // An edit above shifts the reused block, and the region with it.
        d.place_caret(0, false);
        d.insert("Z");
        d.build_visual(80);
        let r3 = d.vmap.regions[0].clone();
        assert_eq!(r3.src, r2.src + 1);
        assert_eq!(
            &d.source[r3.content.clone()],
            "%% note\ngraph TD\n  A-->B\n"
        );
        // Full reveals the fence on the caret's line as its source.
        d.set_markup_mode(MarkupMode::Full);
        d.place_caret(r3.src, false);
        d.build_visual(80);
        assert!(drawn_rows(&d).iter().any(|row| row.contains("A-->B")));
        assert!(d.vmap.regions.is_empty());
    }

    #[test]
    fn an_empty_owned_fence_has_empty_content_at_its_closing_fence() {
        let src = "```mermaid\n```\n\nend\n";
        let mut d = doc_in(View::Wysiwyg, "fence_empty", src);
        d.set_owned_regions(&["fence:mermaid"]);
        caret_at(&mut d, "end");
        let r = d.vmap.regions[0].clone();
        assert_eq!(r.content, 11..11);
        d.edit(r.content.start, r.content.end, "graph LR\n");
        assert_eq!(d.source, "```mermaid\ngraph LR\n```\n\nend\n");
    }

    #[test]
    fn an_owned_inline_directive_is_an_atom_and_an_inline_region() {
        let src = "Due :date[2026-10-12] and :date[2026-11-01]{tz=UTC} then :tag[x] done\n\nend\n";
        let mut d = doc_in(View::Wysiwyg, "inline_directive_owned", src);
        d.set_inline_pictures(true);
        caret_at(&mut d, "end");
        let plain = drawn_rows(&d);
        assert!(plain[0].contains("2026-10-12"), "{plain:?}");
        assert!(d.vmap.regions.is_empty());
        d.set_owned_regions(&["directive:date"]);
        caret_at(&mut d, "end");
        let rows = drawn_rows(&d);
        assert_eq!(rows[0], "Due ▤ and ▤ then x done", "{rows:?}");
        let [a, b] = d
            .vmap
            .regions
            .clone()
            .try_into()
            .unwrap_or_else(|v| panic!("two regions: {v:?}"));
        for r in [&a, &b] {
            assert_eq!(r.kind, "directive:date");
            assert!(r.inline && !r.display);
            assert_eq!(r.rows_span, 0..1);
            assert_eq!(
                r.content,
                r.src..r.end,
                "an inline directive's content is its whole source"
            );
        }
        assert_eq!(&d.source[a.src..a.end], ":date[2026-10-12]");
        assert_eq!(&d.source[b.src..b.end], ":date[2026-11-01]{tz=UTC}");
        let mark = a.directive.as_ref().expect("its directive");
        assert_eq!(
            (mark.name.as_str(), mark.label.as_str()),
            ("date", "2026-10-12")
        );
        let mark = b.directive.as_ref().expect("its directive");
        assert_eq!(
            mark.attrs,
            vec![("tz".to_string(), Some("UTC".to_string()))]
        );
        // The atom is one stop: the caret steps over the whole directive.
        d.place_caret(a.src, false);
        d.move_right(false);
        assert_eq!(d.caret, a.end);
        d.move_left(false);
        assert_eq!(d.caret, a.src);
        // An edit through its content is an edit of the directive.
        let at = a.src + ":date[2026-10-".len();
        d.edit(at, at + 2, "31");
        d.build_visual(80);
        assert_eq!(
            d.vmap.regions[0].directive.as_ref().unwrap().label,
            "2026-10-31"
        );
        // An edit above shifts the reused block, and the regions with it.
        d.place_caret(0, false);
        d.insert("Z");
        d.build_visual(80);
        assert_eq!(
            &d.source[d.vmap.regions[1].src..d.vmap.regions[1].end],
            ":date[2026-11-01]{tz=UTC}"
        );
        // Full shows the caret's line as an unowned directive is drawn: its
        // label, to be typed into.
        d.set_markup_mode(MarkupMode::Full);
        caret_at(&mut d, "Due");
        let line = &drawn_rows(&d)[0];
        assert!(
            line.contains("2026-10-31") && !line.contains('▤'),
            "{line:?}"
        );
        assert!(d.vmap.regions.is_empty());
    }

    #[test]
    fn an_inline_directive_draws_as_before_where_it_is_not_owned_or_cannot_be_a_picture() {
        let src = "Due :date[2026-10-12] and :vis{.family} done\n\nend\n";
        // Not owned, on a picture surface (the web, Apple).
        let mut d = doc_in(View::Wysiwyg, "inline_directive_unowned", src);
        d.set_inline_pictures(true);
        caret_at(&mut d, "end");
        let before = drawn_rows(&d);
        d.set_owned_regions(&["directive:other", "math"]);
        caret_at(&mut d, "end");
        assert_eq!(drawn_rows(&d), before);
        assert!(d.vmap.regions.is_empty());
        // Owned, on a surface that paints nothing in a line (the terminal,
        // Android): drawn as it always was.
        let mut t = doc_in(View::Wysiwyg, "inline_directive_terminal", src);
        caret_at(&mut t, "end");
        let terminal = drawn_rows(&t);
        t.set_owned_regions(&["directive:date", "directive:vis"]);
        caret_at(&mut t, "end");
        assert_eq!(drawn_rows(&t), terminal);
        assert!(t.vmap.regions.is_empty());
        assert!(
            terminal[0].contains("2026-10-12") && terminal[0].contains("⧉ vis"),
            "{terminal:?}"
        );
    }

    #[test]
    fn an_owned_inline_directive_wraps_with_its_atom() {
        let words = "word ".repeat(30);
        let src = format!("{words}:date[2026-10-12] {words}\n");
        let mut d = doc_in(View::Wysiwyg, "inline_directive_wrapped", &src);
        d.set_inline_pictures(true);
        d.set_owned_regions(&["directive:date"]);
        d.place_caret(0, false);
        d.build_visual(40);
        let [r] = d
            .vmap
            .regions
            .clone()
            .try_into()
            .unwrap_or_else(|v| panic!("one region: {v:?}"));
        let row = &d.vmap.rows[r.rows_span.start];
        assert!(
            row.glyphs.iter().any(|g| g.ch == '▤' && g.src == r.src),
            "the region's row lacks its atom"
        );
        assert_eq!(&d.source[r.src..r.end], ":date[2026-10-12]");
    }

    #[test]
    fn an_owned_directive_is_a_region_of_its_whole_source() {
        let src = "intro\n\n::embed[Card]{src=\"a.md\" wide}\n\n::toc\n\n::page-break\n\nend\n";
        let mut d = doc_in(View::Wysiwyg, "directive_owned", src);
        caret_at(&mut d, "end");
        let before = drawn_rows(&d);
        assert!(d.vmap.regions.is_empty());
        d.set_owned_regions(&["directive:embed", "directive:page-break"]);
        caret_at(&mut d, "end");
        assert_eq!(
            drawn_rows(&d),
            before,
            "owning a directive changed its rows"
        );
        let [r] = d
            .vmap
            .regions
            .clone()
            .try_into()
            .unwrap_or_else(|v| panic!("one region: {v:?}"));
        assert_eq!(r.kind, "directive:embed");
        assert!(!r.inline && !r.display);
        assert_eq!(&d.source[r.src..r.end], "::embed[Card]{src=\"a.md\" wide}");
        assert_eq!(r.content, r.src..r.end);
        let mark = r.directive.as_ref().expect("its directive");
        assert_eq!(mark.name, "embed");
        assert_eq!(mark.label, "Card");
        assert_eq!(
            mark.attrs,
            vec![
                ("src".to_string(), Some("a.md".to_string())),
                ("wide".to_string(), Some(String::new()))
            ]
        );
        assert_eq!(r.rows_span, d.vmap.directives[0].rows_span);
        // The caret rests either side of it, as before.
        d.place_caret(r.src, false);
        d.move_right(false);
        assert_eq!(d.caret, r.end);
        // An edit through its content is an edit of the directive.
        let at = r.src + "::embed[".len();
        d.edit(at, at + 4, "Album");
        d.build_visual(80);
        let r2 = d.vmap.regions[0].clone();
        assert_eq!(r2.directive.expect("its directive").label, "Album");
        assert_eq!(&d.source[r2.content], "::embed[Album]{src=\"a.md\" wide}");
    }

    #[test]
    fn a_fence_inside_a_quote_is_not_owned() {
        let src = "> ```mermaid\n> graph TD\n> ```\n\nend\n";
        let mut d = doc_in(View::Wysiwyg, "fence_quoted", src);
        d.set_owned_regions(&["fence:mermaid"]);
        caret_at(&mut d, "end");
        assert!(d.vmap.regions.is_empty(), "{:?}", drawn_rows(&d));
        assert!(drawn_rows(&d).iter().any(|r| r.contains("graph TD")));
    }

    #[test]
    fn set_math_rows_reserves_filler_rows_by_tex() {
        let mut d = doc_in(View::Wysiwyg, "math_rows", "$$\nx\n$$\n\nend\n");
        caret_at(&mut d, "end");
        assert_eq!(d.vmap.math[0].rows_span, 0..1);
        d.set_math_rows(HashMap::from([(d.vmap.math[0].tex.clone(), 3)]));
        d.build_visual(80);
        assert_eq!(d.vmap.math[0].rows_span, 0..3);
        assert_eq!(drawn_rows(&d)[..3], ["∑ x", "", ""]);
        // Cheap when nothing changed.
        let key = d.visual_key();
        d.set_math_rows(HashMap::from([(d.vmap.math[0].tex.clone(), 3)]));
        d.build_visual(80);
        assert_eq!(d.visual_key(), key);
    }

    #[test]
    fn a_dollar_typed_in_shortcuts_authors_math() {
        let mut d = doc_in(View::Wysiwyg, "math_dollar_sc", "\n");
        d.set_markup_mode(MarkupMode::Shortcuts);
        d.caret = 0;
        d.insert("$x$");
        assert_eq!(d.source, "$x$\n");
        d.caret = 1;
        assert_eq!(d.breadcrumb(), "doc › para › inline_math");

        // `None` keeps typed syntax literal, and under the math extension a
        // `$` is syntax: twig escapes it, and the paragraph stays text.
        let mut d = doc_in(View::Wysiwyg, "math_dollar_none", "\n");
        assert_eq!(d.markup_mode(), MarkupMode::None);
        d.caret = 0;
        d.insert("$x$");
        assert_eq!(d.source, "\\$x\\$\n");
        d.caret = 2;
        assert_eq!(d.breadcrumb(), "doc › para › str");
    }

    #[test]
    fn counts_see_a_formula_as_a_picture_however_it_is_written() {
        let c = counts_of(
            "counts_math",
            "the sum $\\sum_i x_i$ and\n\n$$\ny = mx + c\n$$\n",
        );
        // `the sum … and` is three words; neither formula counts, and the
        // display block is not a paragraph of text.
        assert_eq!(c.words, 3);
        assert_eq!(c.paragraphs, 1);
    }
}

#[cfg(test)]
mod verse_tests {
    use super::*;

    fn md(src: &str) -> Doc {
        Doc::from_source(src.into(), Format::Markdown).unwrap()
    }

    #[test]
    fn verse_is_a_capability_of_markdown_djot_and_asciidoc_not_html() {
        assert!(Capabilities::of(Format::Markdown).verse);
        assert!(Capabilities::of(Format::Djot).verse);
        assert!(Capabilities::of(Format::Asciidoc).verse);
        assert!(!Capabilities::of(Format::Html).verse);
    }

    #[test]
    fn toggle_verse_wraps_the_caret_s_paragraph_and_keeps_the_caret_on_its_line() {
        let mut d = md("Roses are red,\nviolets blue.\n");
        d.caret = "Roses are red,\nviol".len();
        d.toggle_verse();
        assert_eq!(
            d.source,
            "<div class=\"verse\">\n\nRoses are red,\\\nviolets blue.\n\n</div>\n"
        );
        assert_eq!(&d.source[d.caret..d.caret + 3], "ets");
        // And off again, with the caret still on `violets`.
        d.toggle_verse();
        assert_eq!(d.source, "Roses are red,\nviolets blue.\n");
        assert_eq!(&d.source[d.caret..d.caret + 3], "ets");
    }

    #[test]
    fn enter_in_a_verse_makes_a_line_and_typing_on_it_keeps_the_stanza() {
        let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
        d.caret = d.source.find("one").unwrap() + 3;
        d.newline();
        // The `\` break became a blank line with the caret on it…
        assert_eq!(d.source, "<div class=\"verse\">\n\none\n\ntwo\n\n</div>\n");
        assert_eq!(d.caret, d.source.find("one").unwrap() + 4);
        // …which the first character makes a line of the same stanza.
        d.insert("x");
        assert_eq!(d.source, "<div class=\"verse\">\n\none\nx\ntwo\n\n</div>\n");
    }

    #[test]
    fn enter_mid_line_in_a_verse_splits_the_line() {
        let mut d = md("<div class=\"verse\">\n\nonetwo\n\n</div>\n");
        d.caret = d.source.find("two").unwrap();
        d.newline();
        assert_eq!(d.source, "<div class=\"verse\">\n\none\ntwo\n\n</div>\n");
    }

    #[test]
    fn backspace_at_a_verse_line_s_start_joins_it_and_takes_the_break_with_it() {
        let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
        d.caret = d.source.find("two").unwrap();
        d.backspace();
        assert_eq!(d.source, "<div class=\"verse\">\n\nonetwo\n\n</div>\n");
        assert_eq!(d.caret, d.source.find("two").unwrap());
    }

    #[test]
    fn backspace_at_a_verse_s_first_line_turns_it_back_into_prose() {
        let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
        d.caret = d.source.find("one").unwrap();
        d.backspace();
        assert_eq!(d.source, "one\ntwo\n");
    }

    #[test]
    fn tab_in_a_verse_indents_by_an_em_space_and_shift_tab_takes_it_back() {
        let mut d = md("<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
        d.caret = d.source.find("two").unwrap() + 1;
        d.indent();
        assert_eq!(
            d.source,
            "<div class=\"verse\">\n\none\\\n\u{2003}two\n\n</div>\n"
        );
        assert_eq!(&d.source[d.caret..d.caret + 2], "wo");
        d.outdent();
        assert_eq!(d.source, "<div class=\"verse\">\n\none\\\ntwo\n\n</div>\n");
        assert_eq!(&d.source[d.caret..d.caret + 2], "wo");
    }
}
