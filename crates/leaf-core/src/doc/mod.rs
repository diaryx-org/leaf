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
    Alignment, BlockContainerKind, BlockKind, Change, Editor, FlatNode, Gesture, InlineKind, Kind,
    NodeId, Op, QueryMatch,
};
use unicode_segmentation::GraphemeCursor;

use crate::Format;
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
mod ops;
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
    /// Every change to the text since a host last took them, as ops against
    /// [`ops_base`](Self::ops_base) — see [`take_ops`](Self::take_ops).
    ops: Vec<Op>,
    /// The text as a host last took the ops, which `ops` read.
    ops_base: String,
    /// twig's revision as of the last change folded into `ops`.
    ops_revision: u64,
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
/// They are Markdown's features; no other language leaf opens declares any, so
/// every other format's mask is empty. Threading this through every
/// constructor (not just `open`) keeps `from_source`, `blank`, and `reload`
/// parsing the same document the same way — twig reparses with these same
/// flags after each edit.
pub(crate) fn parse_extensions(format: Format) -> u32 {
    const FEATURES: &[&str] = &[
        "html_elements",
        "directives",
        "highlight",
        "highlight_colors",
        "math",
    ];
    // `UnsupportedFormat` is a language with no features at all — the empty
    // mask is exactly what it parses with.
    format.twig().feature_flags(FEATURES).unwrap_or(0)
}

/// Build an editor over `bytes` in `format` with leaf's [`parse_extensions`],
/// mapping twig's error into the `anyhow` context every constructor shares.
fn new_editor(bytes: &[u8], format: Format) -> Result<Editor> {
    Editor::new_with_flags(bytes, format.twig(), parse_extensions(format))
        .map_err(|e| anyhow!("twig parse: {e}"))
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
        let flags = parse_extensions(format);
        let supports = |g| format.supports_with(flags, g);
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
mod tests;
