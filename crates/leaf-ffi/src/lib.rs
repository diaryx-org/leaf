//! leaf-ffi — the Swift / C-ABI frontend binding for leaf.
//!
//! This is the native-Apple analogue of `leaf-wasm`: it takes `leaf-core`'s
//! frontend-neutral [`Doc`] — the byte-offset caret model and the AST→glyph
//! [`VisualMap`] — and exposes it across a C ABI (via UniFFI) in the shape an
//! AppKit/SwiftUI renderer wants. Core stays the single source of truth for the
//! text, the caret math, and the offset⇄position mapping; the Swift side only
//! paints glyphs and forwards key/mouse events back in, exactly as the TUI, gpui,
//! and wasm frontends do.
//!
//! ## The boundary is style *runs*, not glyphs
//!
//! [`Doc::build_visual`] resolves the document to rows of per-character glyphs,
//! each tagged with a semantic [`Role`] and the author's emphasis. Sending one
//! object per character would make every keystroke O(document) in boundary
//! crossings. Instead [`LeafDoc::view`] coalesces each row's glyphs into maximal
//! **runs** of identical style and ships those — a handful of records per line.
//! The Swift renderer maps each run's `role` to a font/size/weight and its
//! emphasis flags to traits, the native counterpart of the TUI's `to_ratatui`
//! and the web's CSS class.
//!
//! ## Core owns the grid; Swift owns the pixels
//!
//! Core lays a row out in whole character *columns* (a terminal-cell measure),
//! and every offset⇄position method speaks that grid. It deliberately does *not*
//! dictate presentation. So a native renderer is *proportional* — body text in a
//! real family, headings by **size** and weight, code in a monospace panel — and
//! never multiplies `col × cell_width`. It lets `NSLayoutManager` / Core Text
//! shape each row, places the caret at [`DocView::caret_ch`] (a UTF-16 offset,
//! which is exactly what `NSAttributedString` and `NSTextView` count in), and
//! hit-tests a click through `characterIndex(for:)`, feeding the resulting
//! row + UTF-16 offset back through [`LeafDoc::click_ch`]. Core measures nothing
//! in pixels; Swift positions nothing in the model.
//!
//! ## Threading
//!
//! A UniFFI object is handed to Swift as a reference-counted handle whose methods
//! take `&self`, so the [`Doc`] lives behind a [`Mutex`]. Every call locks, edits
//! or reads, and returns a fresh [`DocView`] — one boundary crossing both mutates
//! and repaints, same as the wasm frontend. Drive it from the main thread.

use std::sync::{Arc, Mutex};

use leaf_core::style::{Baseline, Role, Style as LStyle};
use leaf_core::wysiwyg::text_width;
use leaf_core::{
    Alignment, BlockKind, Capabilities as CoreCapabilities, ColorScheme, Doc, Format, InlineKind,
    LineFlow as CoreLineFlow, MarkupMode as CoreMarkupMode, MediaKind as CoreMediaKind, View,
    VisualMap,
};
use unicode_segmentation::UnicodeSegmentation;

uniffi::setup_scaffolding!();

/// A parse failure constructing a document — the only fallible entry point. Every
/// other method is infallible (it operates on an already-parsed model), so they
/// return a [`DocView`] directly.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum LeafError {
    /// The `format` string handed to [`LeafDoc::new`] wasn't one leaf understands.
    #[error("unknown format: {name}")]
    UnknownFormat { name: String },
    /// `leaf-core` failed to parse `source` as the requested format.
    #[error("parse error: {message}")]
    Parse { message: String },
}

/// A selection cited out of the source — the text, a little of what
/// surrounded it, and the byte range it came from. The FFI shape of
/// `leaf_core::Quote`; see [`LeafDoc::selection_quote`].
#[derive(uniffi::Record)]
pub struct SelectionQuote {
    /// The selected source, verbatim.
    pub exact: String,
    /// What immediately preceded it — empty at the document's start.
    pub prefix: String,
    /// What immediately followed it — empty at the document's end.
    pub suffix: String,
    /// Byte offset in the source where the selection begins.
    pub start: u64,
    /// Byte offset where it ends (exclusive).
    pub end: u64,
}

/// A host-painted range of the source — an annotation's footprint, a search
/// hit. The FFI shape of `leaf_core::Highlight`; see
/// [`LeafDoc::set_highlights`].
#[derive(uniffi::Record)]
pub struct Highlight {
    /// Byte offset in the source where the wash begins.
    pub start: u64,
    /// Byte offset where it ends (exclusive).
    pub end: u64,
    /// The host's name for it, handed back on activation. Opaque to leaf.
    pub id: String,
    /// A rendering hint (`#RRGGBB`), or `None` for the theme's default wash.
    pub color: Option<String>,
    /// A margin glyph's name (an SF Symbol, for this binding's frontends), or
    /// `None` for wash-only ink. The marker — not the wash — is what
    /// activates a highlight; see `leaf_core::Highlight::marker`.
    pub marker: Option<String>,
}

/// One maximal span of same-styled glyphs on a visual row — the unit the Swift
/// renderer turns into a single styled attributed-string run.
#[derive(uniffi::Record)]
pub struct Run {
    /// The run's text, glyphs concatenated in column order.
    pub text: String,
    /// The glyph's semantic role as a renderer class id: `body`, `h1`…`h6`,
    /// `code`, `link`, `mark`, `list`, `quote`, `rule`.
    pub role: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Raised off the baseline and drawn smaller — a footnote reference's `[1]`,
    /// or an author's `^x^`. Mutually exclusive with [`sub`](Self::sub); core's
    /// `Baseline` is one value, and these are its two non-default cases flattened
    /// to the flag shape the rest of this record is spelled in.
    pub sup: bool,
    /// Lowered off the baseline and drawn smaller — an author's `~x~`.
    pub sub: bool,
    /// The byte offset in the source this run's first glyph came from.
    ///
    /// What a run *means*, as opposed to how it looks: a `link` role says a span
    /// is drawn as a link but not where it points, and the only way back to that
    /// is the source. A frontend drawing part of the document somewhere the caret
    /// isn't — a footnote's text in a popover — pairs this with
    /// [`LeafDoc::link_destination_at`] or [`LeafDoc::footnote_at`] to make those
    /// runs followable.
    ///
    /// The alternative was for a frontend to count its way along the row's text
    /// and ask [`LeafDoc::offset_for_pos`], which means converting between three
    /// units that only agree on ASCII: this is a byte offset, the run's text is
    /// characters, and a row's column is a *display* cell (a wide CJK glyph is
    /// two). Handing the offset over is exact, O(1), and needs none of that.
    ///
    /// `0` for the runs of the source view, whose rows are split from raw text
    /// rather than laid out from glyphs.
    pub src: u32,
    /// Whether this run lies inside the active selection — so the renderer can
    /// paint a selection background without re-deriving it from offsets.
    pub sel: bool,
    /// The id of the host highlight covering this run, if one does — see
    /// [`LeafDoc::set_highlights`]. A highlight splits a run the way the
    /// selection does, so a wash begins and ends exactly on its bytes.
    pub hl: Option<String>,
    /// That highlight's rendering hint (`#RRGGBB`, or `None` for the theme's
    /// default wash), carried beside the id so a renderer needs no lookup.
    pub hl_color: Option<String>,
}

/// Where a locator lands — what [`LeafDoc::locate`] answers with, and the FFI
/// mirror of [`leaf_core::Landing`].
///
/// A span rather than an offset because the two things a host does with a
/// locator want different halves of it: following one puts a caret at `start`,
/// while peeking at one draws the rows between `start` and `end`. Only the first
/// can be recovered from an offset alone.
#[derive(uniffi::Record)]
pub struct LandingView {
    /// The first byte of the block the locator names — where a caret goes.
    pub start: u32,
    /// One past its last byte, so the pair maps through `pos_for_offset` to the
    /// rendered rows the block occupies, the way [`FootnoteView`]'s pair does.
    pub end: u32,
}

impl From<leaf_core::Landing> for LandingView {
    fn from(l: leaf_core::Landing) -> Self {
        LandingView {
            start: l.start as u32,
            end: l.end as u32,
        }
    }
}

/// A footnote reference and the note it names — what [`LeafDoc::footnote_at`]
/// answers with. The FFI mirror of [`leaf_core::FootnoteRef`].
///
/// A reference whose definition the document is missing still comes back, with
/// its `label` and no `text`: that a `[^99]` names nothing is a thing to tell
/// the reader, and it is not the same as the caret standing on no reference at
/// all (which is `None`).
#[derive(uniffi::Record)]
pub struct FootnoteView {
    /// The reference's label — the `1` of `[^1]`, without the `^` or brackets.
    pub label: String,
    /// The note's body as source text, or `None` when nothing defines it.
    pub text: Option<String>,
    /// The byte offset the note's body starts at, for a "go to note" that moves
    /// the caret there. `None` alongside a `None` `text`.
    pub offset: Option<u32>,
    /// Where the body ends, exclusive. With `offset` this bounds the note, so a
    /// frontend can map the pair through `pos_for_offset` to the *rendered rows*
    /// it occupies and draw those — the note with its markup resolved, rather
    /// than the asterisks and backticks `text` carries. `None` alongside a
    /// `None` `offset`.
    pub end: Option<u32>,
}

impl From<leaf_core::FootnoteRef> for FootnoteView {
    fn from(f: leaf_core::FootnoteRef) -> Self {
        FootnoteView {
            label: f.label,
            text: f.text,
            offset: f.offset.map(|o| o as u32),
            end: f.end.map(|o| o as u32),
        }
    }
}

/// A footnote definition and the reference that sends a reader to it — what
/// [`LeafDoc::footnote_definition_at_caret`] answers with, and the FFI mirror of
/// [`leaf_core::FootnoteDef`].
///
/// The other half of [`FootnoteView`]'s round trip: that one carries a reader
/// down to the note, this one carries them back up. A definition nothing cites
/// still comes back, with its `label` and no `offset`, for the reason an
/// undefined reference does — "nothing refers to this note" is worth saying.
#[derive(uniffi::Record)]
pub struct FootnoteDefView {
    /// The definition's label — the `1` of `[^1]: …`, spelled exactly as
    /// [`FootnoteView::label`] spells the same footnote's.
    pub label: String,
    /// The byte offset the first reference starts at, for a "back to reference"
    /// that moves the caret there. `None` for a note nothing refers to.
    pub offset: Option<u32>,
}

impl From<leaf_core::FootnoteDef> for FootnoteDefView {
    fn from(f: leaf_core::FootnoteDef) -> Self {
        FootnoteDefView {
            label: f.label,
            offset: f.offset.map(|o| o as u32),
        }
    }
}

/// One visual line: its styled runs plus the row-level flags a frontend draws
/// chrome from.
#[derive(uniffi::Record)]
pub struct Row {
    pub runs: Vec<Run>,
    /// Drawn but holds no caret (a table rule, a block-gap blank line): the
    /// renderer skips it for click/caret math. See [`leaf_core::VRow`].
    pub decoration: bool,
    /// A fenced/indented code-block line — the renderer draws a tinted, bordered
    /// panel around each maximal run of these.
    pub code: bool,
    /// A fenced block's language, carried on the block's first code row only.
    pub code_lang: Option<String>,
    /// A `:::name{.class}` directive-container line — the renderer draws a
    /// tinted panel around each maximal run of these, the `code` recipe. See
    /// [`leaf_core::VRow::directive`].
    pub directive: bool,
    /// A directive container's space-joined `.class` attrs, carried on the
    /// block's first row only. See [`leaf_core::VRow::directive_label`].
    pub directive_label: Option<String>,
    /// The heading level (1–6) if this row belongs to a heading block, else
    /// `None`. A proportional renderer sizes the *whole* row from this so an
    /// inline `` `code` `` run inside a heading still reads at the heading's size.
    pub heading: Option<u8>,
    /// What this row divides, on the blank rows a block boundary is drawn with
    /// and `None` everywhere else — so `boundary != nil` is exactly "this row is
    /// a drawn block boundary". A frontend spaces a boundary by the pair it
    /// falls between (the margin above a heading is wider than the one between
    /// two paragraphs); the *height* is the frontend's, the *kind* is core's.
    /// See [`leaf_core::Boundary`].
    pub boundary: Option<Boundary>,
}

/// What a drawn block boundary separates. The FFI mirror of
/// [`leaf_core::Boundary`].
#[derive(uniffi::Record)]
pub struct Boundary {
    pub above: BlockClass,
    pub below: BlockClass,
}

/// The block kinds core tells apart — the vocabulary a [`Boundary`] is spelled
/// in. The FFI mirror of [`leaf_core::BlockClass`]; `Other` covers every kind
/// core doesn't separate out, so a frontend's `match` stays exhaustive as the
/// list grows.
#[derive(uniffi::Enum)]
pub enum BlockClass {
    Paragraph,
    Heading,
    /// A whole list. Core draws no boundary row *between* two items of one list,
    /// tight or loose, so an `ListItem`↔`ListItem` pair never reaches a frontend.
    List,
    ListItem,
    Quote,
    Code,
    Table,
    Media,
    Directive,
    Rule,
    Footnote,
    Other,
}

impl From<leaf_core::BlockClass> for BlockClass {
    fn from(k: leaf_core::BlockClass) -> Self {
        use leaf_core::BlockClass as K;
        match k {
            K::Paragraph => BlockClass::Paragraph,
            K::Heading => BlockClass::Heading,
            K::List => BlockClass::List,
            K::ListItem => BlockClass::ListItem,
            K::Quote => BlockClass::Quote,
            K::Code => BlockClass::Code,
            K::Table => BlockClass::Table,
            K::Media => BlockClass::Media,
            K::Directive => BlockClass::Directive,
            K::Rule => BlockClass::Rule,
            K::Footnote => BlockClass::Footnote,
            K::Other => BlockClass::Other,
        }
    }
}

/// One *visual line* of a table cell: its styled runs and the source offsets
/// bounding it. A cell is usually one line, but an in-cell hard break (an inline
/// `<br>`) splits it into several — each its own line here, so the frontend
/// shapes and caret-maps them independently (the byte↔UTF-16 offset math a cell
/// needs holds within a line, which carries no break). The runs are *unwrapped*:
/// column width — and any soft wrap within it — is the frontend's to decide.
#[derive(uniffi::Record)]
pub struct TableCellLineView {
    pub runs: Vec<Run>,
    /// The source offsets bounding this line's content — the caret home at its
    /// start and the stop just past its end.
    pub start: u32,
    pub end: u32,
}

/// One cell of a table's structural grid: its content as one or more visual
/// lines, the column alignment its text honours, and the source range the whole
/// cell occupies (where a click or the caret lands).
#[derive(uniffi::Record)]
pub struct TableCellView {
    /// The cell's lines, in order — one unless an in-cell `<br>` splits it.
    pub lines: Vec<TableCellLineView>,
    /// `"left"`, `"right"`, `"center"`, or `"default"`.
    pub align: String,
    /// The source offsets bounding the cell's content — the caret anchors a
    /// click in the cell resolves to.
    pub start: u32,
    pub end: u32,
}

/// One row of a table's structural grid; a header row draws bold and is ruled
/// off from the body below it.
#[derive(uniffi::Record)]
pub struct TableRowView {
    pub head: bool,
    pub cells: Vec<TableCellView>,
}

/// A table described *structurally* rather than as the monospace box-glyph
/// picture that spells it in [`DocView::rows`]. A proportional renderer draws its
/// own grid from this — columns sized to content, real borders — and SKIPS the
/// picture rows in `[start_row, end_row)`. The two describe the same cells at the
/// same source offsets, so the caret lands identically either way. See
/// [`leaf_core::TableInfo`].
#[derive(uniffi::Record)]
pub struct TableView {
    /// The [`DocView::rows`] indices the box-drawn picture occupies — the rows a
    /// grid-drawing frontend skips.
    pub start_row: u32,
    pub end_row: u32,
    pub grid: Vec<TableRowView>,
}

/// A leaf directive (`::name{…}`) — a standalone block with no body, drawn in
/// [`DocView::rows`] as a one-row `⧉ name` placeholder. A frontend that knows
/// the host app's vocabulary reads this and paints the real thing over the rows
/// in `[start_row, end_row)` — a web view for diaryx's `::embed{src=…}`, say —
/// exactly as a grid-drawing one replaces a [`TableView`]'s picture rows. One
/// that doesn't just paints the placeholder, which is already framed by the
/// directive panel chrome.
///
/// Core resolves nothing here and neither does this layer: the vocabulary
/// belongs to the app. See [`leaf_core::DirectiveInfo`].
#[derive(uniffi::Record)]
pub struct DirectiveView {
    /// The [`DocView::rows`] indices the placeholder occupies.
    pub start_row: u32,
    pub end_row: u32,
    /// The directive's type (`embed`, `toc`, `vis`), no leading colons.
    pub name: String,
    /// Its `[label]` text, or empty — what the placeholder row shows.
    pub label: String,
    /// Its `{…}` attributes in source order. A bare attribute (`{public}`) has an
    /// empty value, which a consumer reads as a flag.
    pub attrs: Vec<DirectiveAttr>,
}

/// One `{key=value}` attribute of a [`DirectiveView`]. A record rather than a
/// tuple because UniFFI has no tuple type; an absent value flattens to `""`,
/// since a bare attribute is a flag and the distinction from `key=""` has no
/// consumer on this side.
#[derive(uniffi::Record)]
pub struct DirectiveAttr {
    pub key: String,
    pub value: String,
}

/// What a block-level media placeholder is, so Swift knows which view to build
/// over the rows core reserved: an `NSImageView`/`UIImageView`, or an
/// `AVPlayerView` with or without a picture to show. The peer of
/// [`leaf_core::MediaKind`].
#[derive(uniffi::Enum)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
}

/// One `<source>` alternative of a block media element — a candidate URL plus
/// whichever of the two things HTML picks a `<source>` by: a media query
/// (`<picture>`) or a MIME type (`<video>`/`<audio>`).
///
/// Unlike the web frontend, which hands the whole list to the browser and lets
/// it choose, a native renderer usually wants [`MediaView::src`] — already
/// resolved for the current appearance — and reaches in here only to pick a
/// codec `AVFoundation` can actually play.
#[derive(uniffi::Record)]
pub struct MediaSourceView {
    /// The `media="…"` query, or empty for an unconditional source.
    pub media: String,
    /// The candidate URL (a `<picture>` `srcset` or a `<video>`/`<audio>` `src`).
    pub src: String,
    /// The `type="…"` MIME (`"video/webm"`), or empty when none is declared.
    pub mime: String,
}

/// One block-level image, video, or audio: which rows core reserved for it and
/// what to build there. The peer of [`leaf_core::MediaInfo`], and the media
/// analogue of [`DirectiveView`] — a frontend **skips the rows in
/// `start_row..end_row`** and lays its own view over them, rather than painting
/// the `🖼`/`🎬`/`🔊` placeholder glyphs core put there for a surface that can't.
#[derive(uniffi::Record)]
pub struct MediaView {
    /// The [`DocView::rows`] indices the placeholder occupies.
    pub start_row: u32,
    pub end_row: u32,
    /// Which of the three this is — the view to build.
    pub kind: MediaKind,
    /// The URL to load, already resolved against the current appearance (see
    /// [`LeafDoc::set_dark_appearance`]). A relative path resolves against the
    /// document's own directory, which core does not know — the host does.
    /// Empty only when a `<video>`/`<audio>` named neither a `src` nor a
    /// `<source>`, which is a broken document.
    pub src: String,
    /// A `<video>`'s poster frame URL, or empty. An image destination, so it
    /// loads exactly as an image `src` does — worth showing before the movie is
    /// ready, or in place of one that won't play.
    pub poster: String,
    /// The alt / fallback text, for the view's accessibility label.
    pub alt: String,
    /// The `<source>` alternatives in document order; empty for a plain image.
    pub sources: Vec<MediaSourceView>,
}

/// A per-destination measured height, the way Swift reports one back — the input
/// half of the loop [`LeafDoc::set_media_rows`] closes.
#[derive(uniffi::Record)]
pub struct MediaHeight {
    /// The media's `src` as it appeared in the document, keying it to a
    /// [`MediaView`].
    pub destination: String,
    /// How many visual rows the laid-out view needs.
    pub rows: u32,
}

/// A whole rendered frame: the rows to paint, where the caret sits, and the
/// toolbar state — everything the Swift side needs for one repaint, in one value.
/// Returned by every view-producing method.
#[derive(uniffi::Record)]
pub struct DocView {
    pub rows: Vec<Row>,
    /// Tables described structurally, for a frontend that draws its own grid
    /// instead of painting the box-glyph rows. Empty in the source view. Each
    /// names the `rows` span its picture occupies, to be skipped.
    pub tables: Vec<TableView>,
    /// Leaf directives (`::name{…}`) described structurally, for a frontend that
    /// paints what the host app's vocabulary makes of them instead of the `⧉`
    /// placeholder row. Empty in the source view, where the directive is the
    /// literal text the caret is editing.
    pub directives: Vec<DirectiveView>,
    /// Block-level images, videos, and audio described structurally, for a
    /// frontend that lays real views over the rows core reserved instead of
    /// painting the placeholder glyphs. Empty in the source view, where the
    /// `![](…)` or `<video>` markup is the literal text being edited.
    pub media: Vec<MediaView>,
    /// The caret's row: an index into [`Self::rows`].
    pub caret_row: u32,
    /// The caret's display *column* within its row — core's grid position. Kept
    /// for callers reasoning in columns; a proportional renderer wants
    /// [`Self::caret_ch`] instead.
    pub caret_col: u32,
    /// The caret's offset within its row's text in **UTF-16 code units** — what
    /// `NSAttributedString`/`NSTextView` count to. This is `caret_col` mapped
    /// through the row's grapheme widths, so it lands the caret correctly past
    /// wide glyphs (CJK, emoji) where a column and a character index diverge.
    pub caret_ch: u32,
    /// The caret's **source byte offset** — the coordinate a table cell is keyed
    /// by (`TableCellView::start`/`end`), so a frontend drawing its own grid can
    /// find which cell the caret sits in without the picture-row indices.
    pub caret_src: u32,
    /// Whether a (non-empty) selection is active.
    pub has_selection: bool,
    /// The selection's *fixed* end (the caret is the moving end), as a row and a
    /// UTF-16 offset — so the renderer can restore a native selection with the
    /// same direction the model has. Equal to the caret when `has_selection` is
    /// false.
    pub anchor_row: u32,
    pub anchor_ch: u32,
    /// Whether the buffer differs from the last saved bytes — for a "● modified"
    /// affordance.
    pub dirty: bool,
    /// `"wysiwyg"` or `"source"`, for a view-toggle affordance.
    pub view: String,
    /// The heading level at the caret, if any — a toolbar lights H1…H6 from it.
    pub heading: Option<u32>,
    /// The inline marks active at the caret (`bold`, `italic`, `code`, …) — the
    /// toolbar lights the matching buttons.
    pub active: Vec<String>,
    /// The destination of the link the caret stands in, or `None` — the toolbar
    /// lights its Link button from it and seeds an edit of that link with it.
    ///
    /// It rides the frame rather than being a query a toolbar makes for itself
    /// because a toolbar only redraws when the *state* changes: walking the caret
    /// out of a link changes no mark, no heading, and no dirty flag, so a Link
    /// button reading this by a call of its own would keep a stale light on. Same
    /// reason `heading` is here and not asked for.
    ///
    /// Only a *parsed* link answers ([`LeafDoc::link_destination_at_caret`]);
    /// a wikilink is literal text with no node behind it, and has nothing to
    /// repoint — see `LinkTarget.swift`.
    pub link: Option<String>,
}

/// A visual position: a row index plus a UTF-16 offset within that row's text —
/// the coordinate the geometry side (Core Text) draws from. Returned by
/// [`LeafDoc::pos_for_offset`], the bridge from a source offset (what a
/// `UITextPosition` wraps) to where it sits on screen.
#[derive(uniffi::Record)]
pub struct RowCol {
    pub row: u32,
    pub ch: u32,
}

/// The rows a source range covers, both ends **inclusive** — what a frontend
/// slices out of a frame to draw a block somewhere other than where it sits: a
/// footnote peek, a link peek, a landing flash. Returned by
/// [`LeafDoc::row_range_for`].
///
/// Inclusive rather than half-open because the answer is "these rows", not "up
/// to here": every caller wants `rows[first...last]`, and a `last` one past the
/// end would be a second thing to get wrong at each of them. `last >= first`
/// always, so the pair is never empty — a range with no visible byte still
/// covers the row it opened on.
#[derive(uniffi::Record)]
pub struct RowRange {
    pub first: u32,
    pub last: u32,
}

/// Which formatting controls this document's format can spell — the toolbar's
/// enabled state, one flag per button, from [`LeafDoc::capabilities`]. Mirrors
/// [`leaf_core::Capabilities`], where the reasoning lives.
///
/// Its shape is a flat record of `Bool`s rather than a query taking a gesture
/// because the Swift side wants exactly one crossing and a value it can hold in
/// an `@Observable`: `let caps = doc.capabilities()`, then
/// `.disabled(!caps.bold)` on each control.
#[derive(uniffi::Record)]
pub struct Capabilities {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub mark: bool,
    pub underline: bool,
    pub strike: bool,
    pub superscript: bool,
    pub subscript: bool,
    /// Both [`LeafDoc::set_heading`] and [`LeafDoc::set_paragraph`] — they are
    /// the same gesture in core and stand or fall together.
    pub heading: bool,
    pub blockquote: bool,
    pub bullet_list: bool,
    pub ordered_list: bool,
    /// [`LeafDoc::toggle_task_item`], [`LeafDoc::toggle_task_checked`] and
    /// [`LeafDoc::toggle_task_at`] — including a *tap* on a rendered checkbox,
    /// which should not be live where the box cannot be spelled.
    pub task: bool,
    pub link: bool,
    /// [`LeafDoc::insert_image`] and [`LeafDoc::insert_media`] both.
    pub image: bool,
    pub thematic_break: bool,
    /// [`LeafDoc::insert_footnote`]. Markdown and djot spell the pair; HTML does
    /// not, so the button goes rather than dims into a refusal.
    pub footnote: bool,
    pub code_language: bool,
    /// The grid controls. Gate them on this *and* [`LeafDoc::caret_in_table`]:
    /// this asks whether the format's tables are editable, that whether the
    /// caret is in one.
    pub table: bool,
    /// Shift+Return inside a cell — [`LeafDoc::cell_line_break`].
    pub cell_line_break: bool,
}

impl From<CoreCapabilities> for Capabilities {
    fn from(c: CoreCapabilities) -> Self {
        Self {
            bold: c.bold,
            italic: c.italic,
            code: c.code,
            mark: c.mark,
            underline: c.underline,
            strike: c.strike,
            superscript: c.superscript,
            // `subscript` is a Swift keyword; uniffi escapes it in the generated
            // binding (`caps.`subscript``), so the field keeps its real name
            // here rather than wearing a suffix on both sides of the boundary.
            subscript: c.subscript,
            heading: c.heading,
            blockquote: c.blockquote,
            bullet_list: c.bullet_list,
            ordered_list: c.ordered_list,
            task: c.task,
            link: c.link,
            image: c.image,
            thematic_break: c.thematic_break,
            footnote: c.footnote,
            code_language: c.code_language,
            table: c.table,
            cell_line_break: c.cell_line_break,
        }
    }
}

/// A table column's text alignment — the argument to
/// [`LeafDoc::table_set_alignment`]. Mirrors twig's `Alignment`.
#[derive(uniffi::Enum)]
pub enum TableAlignment {
    Default,
    Left,
    Right,
    Center,
}

impl TableAlignment {
    fn into_core(self) -> Alignment {
        match self {
            TableAlignment::Default => Alignment::Default,
            TableAlignment::Left => Alignment::Left,
            TableAlignment::Right => Alignment::Right,
            TableAlignment::Center => Alignment::Center,
        }
    }
}

/// How much of the source markup the rich view exposes — the argument to
/// [`LeafDoc::set_markup_mode`]. Mirrors [`leaf_core::MarkupMode`]; `None`
/// is the default (the clean surface Diaryx ships, with typed syntax kept
/// literal).
///
/// A single three-way ladder rather than a pair of toggles, because only three
/// of the four combinations of its two axes — reveal the caret's delimiters,
/// author markup from typing — are coherent. See [`leaf_core::MarkupMode`]
/// for which one is left out and why.
#[derive(uniffi::Enum)]
pub enum MarkupMode {
    None,
    Shortcuts,
    Full,
}

impl MarkupMode {
    fn into_core(self) -> CoreMarkupMode {
        match self {
            MarkupMode::None => CoreMarkupMode::None,
            MarkupMode::Shortcuts => CoreMarkupMode::Shortcuts,
            MarkupMode::Full => CoreMarkupMode::Full,
        }
    }

    fn from_core(mode: CoreMarkupMode) -> Self {
        match mode {
            CoreMarkupMode::None => MarkupMode::None,
            CoreMarkupMode::Shortcuts => MarkupMode::Shortcuts,
            CoreMarkupMode::Full => MarkupMode::Full,
        }
    }
}

/// How the rich view treats a soft break (a bare newline inside a paragraph) —
/// the argument to [`LeafDoc::set_line_flow`]. Mirrors [`leaf_core::LineFlow`];
/// `Fold` is the default (soft breaks reflow into the paragraph, as before).
#[derive(uniffi::Enum)]
pub enum LineFlow {
    Fold,
    Preserve,
}

impl LineFlow {
    fn into_core(self) -> CoreLineFlow {
        match self {
            LineFlow::Fold => CoreLineFlow::Fold,
            LineFlow::Preserve => CoreLineFlow::Preserve,
        }
    }

    fn from_core(mode: CoreLineFlow) -> Self {
        match mode {
            CoreLineFlow::Fold => LineFlow::Fold,
            CoreLineFlow::Preserve => LineFlow::Preserve,
        }
    }
}

/// A live leaf document bound for a native Apple frontend: `leaf_core::Doc` plus
/// the wrap width the current viewport implies, behind a mutex. Constructed from
/// an in-memory string and driven entirely through method calls — there is no
/// filesystem behind it.
#[derive(uniffi::Object)]
pub struct LeafDoc {
    inner: Mutex<Inner>,
}

/// The guarded state. Its methods assume the lock is held (they take `&mut
/// self`); the [`LeafDoc`] exported wrappers acquire it, delegate, and return the
/// resulting frame.
struct Inner {
    doc: Doc,
    /// The wrap mode. `Some(cols)` wraps the map at that column budget (a terminal,
    /// or a fixed-cell frontend); `None` builds it **unwrapped** — one row per block —
    /// for a proportional GUI that wraps at its own pixel width. `build_visual`
    /// caches on `(revision, width)`, so re-syncing when neither moved is free.
    width: Option<usize>,
    /// The host's current appearance, which a `<picture>`'s `prefers-color-scheme`
    /// `<source>`s are matched against when resolving a block image's URL. Core
    /// has no theme of its own, so this is AppKit/UIKit answering on its behalf;
    /// defaults to light until the host calls
    /// [`LeafDoc::set_dark_appearance`].
    scheme: ColorScheme,
}

// SAFETY: `Doc` embeds a `twig::Editor`, which holds a `NonNull<TwigEditor>` and
// is therefore `!Send`. UniFFI hands `LeafDoc` to Swift as a reference-counted
// handle that must be `Send + Sync`, so `Inner` must be `Send`. This is sound
// because:
//   1. Every access goes through `LeafDoc::lock()` — the `Mutex` serializes all
//      reads and mutations, so there is never concurrent access to the handle.
//   2. twig's editor handle owns a plain heap allocation with no thread-affinity
//      (no thread-locals, no per-thread state) — moving the pointer between
//      threads is fine as long as use is serialized, which (1) guarantees.
// The intended usage is still main-thread-driven; this impl only permits the
// handle to cross threads safely, it does not invite concurrent use.
unsafe impl Send for Inner {}

impl Inner {
    /// Rebuild the visual map at the current width. Cheap (cached) when nothing
    /// changed; the guard that lets every movement/click method assume a fresh
    /// grid regardless of call order.
    fn sync(&mut self) {
        match self.width {
            Some(w) => self.doc.build_visual(w),
            None => self.doc.build_visual_unwrapped(),
        }
    }

    /// The plain text of visual row `row` in the active view — the string the
    /// renderer concatenates its runs into. Backs the column⇄UTF-16 mapping.
    fn row_text(&self, row: usize) -> String {
        match self.doc.view {
            View::Wysiwyg => self
                .doc
                .vmap
                .rows
                .get(row)
                .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
                .unwrap_or_default(),
            View::Source => self
                .doc
                .source
                .split('\n')
                .nth(row)
                .unwrap_or("")
                .to_string(),
        }
    }

    /// The `(row, display-column)` a source offset sits at in the active view.
    fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.pos_of_offset(off),
            View::Source => {
                let s = &self.doc.source;
                // Walk back to a character boundary, not just into range. Every
                // offset here arrives from a UI toolkit counting in its own
                // units, so one landing mid-character is ordinary input — and
                // slicing on it aborts the process across an FFI boundary that
                // has no unwinding. `snap_stop` and `text_in_range` already do
                // this; this was the one door left open.
                let mut off = off.min(s.len());
                while off > 0 && !s.is_char_boundary(off) {
                    off -= 1;
                }
                let row = s[..off].bytes().filter(|&b| b == b'\n').count();
                let line_start = s[..off].rfind('\n').map_or(0, |i| i + 1);
                (row, text_width(&s[line_start..off]))
            }
        }
    }

    /// The inclusive row span a source range occupies in the active view.
    ///
    /// The rich view defers to [`leaf_core::wysiwyg::VisualMap::row_range_for`],
    /// where the reasoning lives. The source view has no hidden bytes at all —
    /// every byte is drawn on the line it is written on — so counting newlines
    /// is the whole answer, and the last row is the one holding the range's last
    /// byte rather than the one past it.
    fn row_range_for(&self, start: usize, end: usize) -> (usize, usize) {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.row_range_for(start..end),
            View::Source => {
                let first = self.pos_of_offset(start).0;
                let last = self.pos_of_offset(end.max(start.saturating_add(1)) - 1).0;
                (first, last.max(first))
            }
        }
    }

    /// The source offset under a click at row `row`, `ch` UTF-16 units in.
    fn offset_at(&mut self, row: usize, ch: usize) -> usize {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.doc.click(row, col, false);
        self.doc.caret
    }

    // ── position mapping for UITextInput (non-mutating; caret untouched) ───────
    // These branch by view exactly as `pos_of_offset` does, so the WYSIWYG map and
    // the raw-source grid each answer in their own coordinates.

    /// The source offset of display column `col` on visual `row` — the inverse of
    /// [`Self::pos_of_offset`] in column space.
    fn offset_of_col(&self, row: usize, col: usize) -> usize {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.offset_of_pos(row, col),
            View::Source => {
                let line = self.row_text(row);
                let (mut c, mut b) = (0usize, 0usize);
                for g in line.graphemes(true) {
                    if c >= col {
                        break;
                    }
                    c += text_width(g);
                    b += g.len();
                }
                self.source_line_start(row) + b
            }
        }
    }

    /// The byte offset where visual `row` begins in the source view.
    fn source_line_start(&self, row: usize) -> usize {
        self.doc
            .source
            .split('\n')
            .take(row)
            .map(|l| l.len() + 1)
            .sum()
    }

    /// The next caret stop after `off`, or `None` at the end.
    fn stop_after(&self, off: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.stop_after(off),
            View::Source => {
                let s = &self.doc.source;
                if off >= s.len() {
                    None
                } else {
                    Some(
                        s[off..]
                            .grapheme_indices(true)
                            .nth(1)
                            .map_or(s.len(), |(i, _)| off + i),
                    )
                }
            }
        }
    }

    /// The previous caret stop before `off`, or `None` at the start.
    fn stop_before(&self, off: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.stop_before(off),
            View::Source => {
                let s = &self.doc.source;
                let off = off.min(s.len());
                if off == 0 {
                    None
                } else {
                    s[..off].grapheme_indices(true).next_back().map(|(i, _)| i)
                }
            }
        }
    }

    /// Snap `off` to a valid caret stop (WYSIWYG) / char boundary (source).
    fn snap_stop(&self, off: usize) -> usize {
        let s = &self.doc.source;
        let mut off = off.min(s.len());
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.snap_to_stop(off),
            View::Source => {
                while off > 0 && !s.is_char_boundary(off) {
                    off -= 1;
                }
                off
            }
        }
    }

    /// The navigable visual row above `row`, if any.
    fn nav_above(&self, row: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.navigable_above(row),
            View::Source => (row > 0).then(|| row - 1),
        }
    }

    /// The navigable visual row below `row`, if any.
    fn nav_below(&self, row: usize) -> Option<usize> {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.navigable_below(row),
            View::Source => {
                let n = self.doc.source.split('\n').count();
                (row + 1 < n).then_some(row + 1)
            }
        }
    }

    /// Resolve the current document to a renderable frame of style runs. Called
    /// for the first paint, on resize, and by every mutating wrapper so one
    /// boundary crossing both edits and repaints.
    fn view(&mut self) -> DocView {
        self.sync();

        let (ss, se) = self.doc.selection().unwrap_or((usize::MAX, usize::MAX));

        // The two views speak different grids — the WYSIWYG map's resolved glyphs
        // vs the raw source split on newlines — and `caret_pos` branches to match,
        // so the rows must too or the caret lands on the wrong text.
        let rows = match self.doc.view {
            View::Wysiwyg => wysiwyg_rows(&self.doc.vmap, ss, se, self.doc.highlights()),
            View::Source => source_rows(&self.doc.source, ss, se),
        };
        // Structural tables, for a proportional renderer that draws its own grid;
        // none in the source view (the caret rides raw pipe text there).
        let tables = match self.doc.view {
            View::Wysiwyg => wysiwyg_tables(&self.doc.vmap, ss, se, self.doc.highlights()),
            View::Source => Vec::new(),
        };

        // Leaf directives, on the same terms as the tables above: structural in
        // the rich view, absent in the source view.
        let directives = match self.doc.view {
            View::Wysiwyg => wysiwyg_directives(&self.doc.vmap),
            View::Source => Vec::new(),
        };

        // Block media, on the same terms again: only the rich view has
        // placeholder rows to lay a view over.
        let media = match self.doc.view {
            View::Wysiwyg => wysiwyg_media(&self.doc.vmap, self.scheme),
            View::Source => Vec::new(),
        };

        let (caret_row, caret_col) = self.doc.caret_pos();
        // Map the caret's display column to a UTF-16 text offset so a native
        // renderer can place it past wide glyphs (see [`DocView::caret_ch`]).
        let caret_ch = col_to_utf16(&self.row_text(caret_row), caret_col);
        // The selection's fixed (anchor) end, in the same row/UTF-16 terms.
        let (has_selection, anchor_row, anchor_ch) = match self.doc.selection() {
            Some(_) => {
                let a = self.doc.anchor.unwrap_or(self.doc.caret);
                let (ar, ac) = self.pos_of_offset(a);
                (true, ar, col_to_utf16(&self.row_text(ar), ac))
            }
            None => (false, caret_row, caret_ch),
        };
        let heading = self.doc.current_heading_level();
        let active = self
            .doc
            .active_inline_marks()
            .iter()
            .map(|k| mark_id(k).to_string())
            .collect();
        let link = self.doc.link_destination_at_caret();

        DocView {
            rows,
            tables,
            directives,
            media,
            caret_row: caret_row as u32,
            caret_col: caret_col as u32,
            caret_ch: caret_ch as u32,
            caret_src: self.doc.caret.min(self.doc.source.len()) as u32,
            has_selection,
            anchor_row: anchor_row as u32,
            anchor_ch: anchor_ch as u32,
            dirty: self.doc.dirty,
            view: self.doc.view_name().to_string(),
            heading,
            active,
            link,
        }
    }
}

#[uniffi::export]
impl LeafDoc {
    /// Parse `source` as `format` (`"markdown"`/`"md"`, `"djot"`/`"dj"`,
    /// `"html"`, `"xml"`) into a live, untitled document.
    #[uniffi::constructor]
    pub fn new(source: String, format: String) -> Result<Arc<Self>, LeafError> {
        let format = match format.to_ascii_lowercase().as_str() {
            "markdown" | "md" => Format::Markdown,
            "djot" | "dj" => Format::Djot,
            "html" | "htm" => Format::Html,
            "xml" => Format::Xml,
            other => {
                return Err(LeafError::UnknownFormat {
                    name: other.to_string(),
                });
            }
        };
        let doc = Doc::from_source(source, format).map_err(|e| LeafError::Parse {
            message: e.to_string(),
        })?;
        Ok(Arc::new(LeafDoc {
            inner: Mutex::new(Inner {
                doc,
                width: Some(80),
                scheme: ColorScheme::Light,
            }),
        }))
    }

    /// Resolve the current document to a renderable frame — the first paint.
    pub fn view(&self) -> DocView {
        self.lock().view()
    }

    /// Set the wrap width (in columns) the viewport implies and repaint. For a
    /// fixed-cell frontend (a terminal); a proportional GUI uses [`set_unwrapped`].
    pub fn set_width(&self, cols: u32) -> DocView {
        let mut g = self.lock();
        g.width = Some((cols as usize).max(1));
        g.view()
    }

    /// Switch to **unwrapped** layout — one visual row per block, no column wrapping —
    /// and repaint. A proportional GUI calls this once at start-up, then wraps each
    /// row at its own pixel width (the caret/hit/selection geometry it derives from
    /// the pixel wrap; core still owns the caret model, in byte offsets). Idempotent
    /// and cheap to leave in place across edits.
    pub fn set_unwrapped(&self) -> DocView {
        let mut g = self.lock();
        g.width = None;
        g.view()
    }

    /// Tell core whether the host is in a dark appearance, so a `<picture>`'s
    /// `prefers-color-scheme` `<source>`s resolve to the right banner. Call it
    /// from `viewDidChangeEffectiveAppearance` (AppKit) or
    /// `traitCollectionDidChange` (UIKit).
    ///
    /// Cheap to call repeatedly: resolving at the same appearance yields the same
    /// URLs, and a renderer keying its views by `src` tears nothing down.
    pub fn set_dark_appearance(&self, dark: bool) -> DocView {
        let mut g = self.lock();
        g.scheme = if dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        };
        g.view()
    }

    /// Report how many visual rows each block media actually needs, measured from
    /// the views the renderer laid out, keyed by the media's `src`.
    ///
    /// Core does no I/O and can't know how tall a picture or a player is, so this
    /// is the only way a placeholder grows past its default single row. The loop
    /// is: lay out at the current reservation → measure → call this → repaint if
    /// it changed. Handing over the same measurements again is a no-op, so a
    /// renderer can report its current state each frame without diffing first.
    ///
    /// A frontend that lays media out in its own units and simply reserves the
    /// vertical space itself (the way the gpui GUI does with images) never needs
    /// to call this at all.
    pub fn set_media_rows(&self, heights: Vec<MediaHeight>) -> DocView {
        let mut g = self.lock();
        g.doc.set_media_rows(
            heights
                .into_iter()
                .map(|h| (h.destination, h.rows.max(1) as usize))
                .collect(),
        );
        g.view()
    }

    /// Insert a block-level image, video, or audio at the caret. Any selection
    /// becomes the alt / fallback text. See [`leaf_core::Doc::insert_media`] for
    /// the markup each kind spells.
    pub fn insert_media(&self, kind: MediaKind, destination: String, alt: String) -> DocView {
        let mut g = self.lock();
        let kind = match kind {
            MediaKind::Image => CoreMediaKind::Image,
            MediaKind::Video => CoreMediaKind::Video,
            MediaKind::Audio => CoreMediaKind::Audio,
        };
        g.doc.insert_media(kind, &destination, &alt);
        g.view()
    }

    /// Insert a thematic break (`---`) at the caret — the toolbar's Horizontal
    /// Rule button. See [`leaf_core::Doc::insert_thematic_break`] for how it
    /// handles a selection, a blank line, and the caret sitting mid-paragraph,
    /// mid-list, or inside a quote.
    pub fn insert_thematic_break(&self) -> DocView {
        let mut g = self.lock();
        g.doc.insert_thematic_break();
        g.view()
    }

    /// The current source text — for a save (write to disk / iCloud / a document
    /// wrapper) or a source-view display.
    pub fn source(&self) -> String {
        self.lock().doc.source.clone()
    }

    /// The selected text, if any — for a clipboard copy/cut.
    pub fn selected_text(&self) -> Option<String> {
        self.lock().doc.selected_text().map(str::to_string)
    }

    /// The selection as a quote with up to `context` characters of what
    /// surrounded it, cut from the **source** — the shape a host that cites or
    /// annotates a passage wants, findable in the document again by plain
    /// string search. `None` when nothing is selected. See
    /// `leaf_core::Doc::selection_quote`.
    pub fn selection_quote(&self, context: u32) -> Option<SelectionQuote> {
        let g = self.lock();
        g.doc
            .selection_quote(context as usize)
            .map(|q| SelectionQuote {
                exact: q.exact,
                prefix: q.prefix,
                suffix: q.suffix,
                start: q.start as u64,
                end: q.end as u64,
            })
    }

    /// Whether the document refuses to change — see `set_read_only`.
    pub fn read_only(&self) -> bool {
        self.lock().doc.read_only()
    }

    /// Turn the read-only gate on or off — a *reading* surface over the same
    /// rendering, selection and navigation the editor has. Enforced in core at
    /// the three doors every mutation goes through, so a host that also quiets
    /// its input chrome is polishing, not protecting.
    pub fn set_read_only(&self, on: bool) -> DocView {
        let mut g = self.lock();
        g.doc.set_read_only(on);
        g.view()
    }

    /// Replace the host-painted source ranges wholesale and repaint — see
    /// `leaf_core::Doc::set_highlights` for why it is a replace, and
    /// [`Highlight`] for what one is.
    pub fn set_highlights(&self, highlights: Vec<Highlight>) -> DocView {
        let mut g = self.lock();
        let hls = highlights
            .into_iter()
            .map(|h| leaf_core::Highlight {
                start: h.start as usize,
                end: h.end as usize,
                id: h.id,
                color: h.color,
                marker: h.marker,
            })
            .collect();
        g.doc.set_highlights(hls);
        g.view()
    }

    /// The id of the highlight covering source `offset`, if one does — what a
    /// frontend asks when the reader activates a spot on the page.
    pub fn highlight_at(&self, offset: u32) -> Option<String> {
        self.lock()
            .doc
            .highlight_at(offset as usize)
            .map(|h| h.id.clone())
    }

    /// The host-painted ranges as last set, sorted by start — what a frontend
    /// walks to lay out margin markers.
    pub fn highlights(&self) -> Vec<Highlight> {
        self.lock()
            .doc
            .highlights()
            .iter()
            .map(|h| Highlight {
                start: h.start as u64,
                end: h.end as u64,
                id: h.id.clone(),
                color: h.color.clone(),
                marker: h.marker.clone(),
            })
            .collect()
    }

    /// Mark the buffer saved after the host persisted [`LeafDoc::source`] its own
    /// way — clears the dirty flag without touching a filesystem.
    pub fn mark_saved(&self) -> DocView {
        let mut g = self.lock();
        g.doc.mark_saved();
        g.view()
    }

    // ── text input ───────────────────────────────────────────────────────────

    pub fn insert(&self, text: String) -> DocView {
        let mut g = self.lock();
        g.doc.insert(&text);
        g.view()
    }

    pub fn paste(&self, text: String) -> DocView {
        let mut g = self.lock();
        g.doc.paste(&text);
        g.view()
    }

    pub fn newline(&self) -> DocView {
        let mut g = self.lock();
        g.doc.newline();
        g.view()
    }

    /// Tab away from a table: indent the caret's line (or the selected lines) one
    /// level, nesting a list item under its sibling. The frontend calls this when
    /// [`LeafDoc::cell_tab`] declined because the caret isn't in a table.
    pub fn indent(&self) -> DocView {
        let mut g = self.lock();
        g.doc.indent();
        g.view()
    }

    /// Shift+Tab away from a table: take one indent level back off the caret's
    /// line (or the selected lines), unnesting a list item. The mirror of
    /// [`LeafDoc::indent`].
    pub fn outdent(&self) -> DocView {
        let mut g = self.lock();
        g.doc.outdent();
        g.view()
    }

    // ── table keys ────────────────────────────────────────────────────────────
    // Tab, Return, and Shift+Return take on table meanings when the caret is in
    // one. Each returns `Some(view)` when it acted as a table key and `None` when
    // the caret isn't in a table — the frontend then does the key's ordinary job
    // (indent, newline), so these keep their meaning everywhere else.

    /// Tab (`forward`) / Shift+Tab hops to the next/previous cell; Tab past the
    /// last cell appends a fresh row and enters it.
    pub fn cell_tab(&self, forward: bool) -> Option<DocView> {
        let mut g = self.lock();
        g.sync();
        g.doc.cell_tab(forward).then(|| g.view())
    }

    /// Return drops to the cell below in the same column, appending a row at the
    /// table's bottom.
    pub fn cell_return(&self) -> Option<DocView> {
        let mut g = self.lock();
        g.sync();
        g.doc.cell_return().then(|| g.view())
    }

    /// Shift+Return inserts a hard line break *within* the current cell.
    pub fn cell_line_break(&self) -> Option<DocView> {
        let mut g = self.lock();
        g.sync();
        g.doc.cell_line_break().then(|| g.view())
    }

    pub fn backspace(&self) -> DocView {
        let mut g = self.lock();
        g.doc.backspace();
        g.view()
    }

    pub fn delete_forward(&self) -> DocView {
        let mut g = self.lock();
        g.doc.delete_forward();
        g.view()
    }

    pub fn delete_word_back(&self) -> DocView {
        let mut g = self.lock();
        g.doc.delete_word_back();
        g.view()
    }

    pub fn delete_word_forward(&self) -> DocView {
        let mut g = self.lock();
        g.doc.delete_word_forward();
        g.view()
    }

    // ── caret movement ───────────────────────────────────────────────────────
    // Each syncs the grid first (movement reads the stop table / column layout),
    // moves, then repaints — `Inner::view` re-syncs but that's the cached no-op.

    pub fn move_left(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_left(extend);
        g.view()
    }

    pub fn move_right(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_right(extend);
        g.view()
    }

    pub fn move_up(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_up(extend);
        g.view()
    }

    pub fn move_down(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_down(extend);
        g.view()
    }

    pub fn move_word_left(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_word_left(extend);
        g.view()
    }

    pub fn move_word_right(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_word_right(extend);
        g.view()
    }

    pub fn move_home(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_home(extend);
        g.view()
    }

    pub fn move_end(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_end(extend);
        g.view()
    }

    pub fn move_doc_start(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_doc_start(extend);
        g.view()
    }

    pub fn move_doc_end(&self, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.move_doc_end(extend);
        g.view()
    }

    pub fn select_all(&self) -> DocView {
        let mut g = self.lock();
        g.doc.select_all();
        g.view()
    }

    /// Place the caret from a click, in core's column grid: `row` indexes the
    /// visual [`Row`]s and `col` is the glyph column within it. Core clamps both
    /// to real caret stops. Prefer [`LeafDoc::click_ch`] from a proportional
    /// renderer.
    pub fn click(&self, row: u32, col: u32, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        g.doc.click(row as usize, col as usize, extend);
        g.view()
    }

    /// Place the caret from a click whose horizontal position is a **UTF-16
    /// offset** into the visual row's text — what `characterIndex(for:)` hands
    /// back. Converted to core's display column before clicking, so a proportional
    /// renderer never reasons about column widths itself.
    pub fn click_ch(&self, row: u32, ch: u32, extend: bool) -> DocView {
        let mut g = self.lock();
        g.sync();
        let col = utf16_to_col(&g.row_text(row as usize), ch as usize);
        g.doc.click(row as usize, col, extend);
        g.view()
    }

    /// Select the word under a click (row, `ch`) — the double-click gesture.
    pub fn select_word_ch(&self, row: u32, ch: u32) -> DocView {
        let mut g = self.lock();
        let off = g.offset_at(row as usize, ch as usize);
        g.doc.select_word_at(off);
        g.view()
    }

    /// Select the whole logical text block under a click (row, `ch`) — the
    /// triple-click gesture. Grabs the entire block even where it soft-wraps.
    pub fn select_block_ch(&self, row: u32, ch: u32) -> DocView {
        let mut g = self.lock();
        let off = g.offset_at(row as usize, ch as usize);
        g.doc.select_block_at(off);
        g.view()
    }

    /// Mirror a native selection into the model: `[anchor, focus]` given as
    /// row + UTF-16 offset pairs. Each is resolved to a source offset the way a
    /// click is, then set as the selection's fixed and moving ends. A collapsed
    /// range (`anchor == focus`) just places the caret.
    pub fn set_selection(
        &self,
        anchor_row: u32,
        anchor_ch: u32,
        focus_row: u32,
        focus_ch: u32,
    ) -> DocView {
        let mut g = self.lock();
        let anchor = g.offset_at(anchor_row as usize, anchor_ch as usize);
        let focus = g.offset_at(focus_row as usize, focus_ch as usize);
        g.doc.place_caret(anchor, false);
        if anchor != focus {
            g.doc.place_caret(focus, true);
        }
        g.view()
    }

    // ── rich clipboard (mirrors leaf-tui / leaf-gpui / leaf-wasm) ─────────────

    /// The current selection rendered to HTML by twig — the rich flavor a copy
    /// writes alongside the plain [`LeafDoc::selected_text`]. `None` when nothing
    /// is selected.
    pub fn selection_html(&self) -> Option<String> {
        self.lock().doc.selection_html()
    }

    /// Paste, preferring the clipboard's rich (`text/html`) flavor: twig parses
    /// `html` into the document's own markup and inserts it. Falls back to the
    /// plain `text` when there's no HTML or it doesn't parse.
    pub fn paste_rich(&self, html: Option<String>, text: String) -> DocView {
        let mut g = self.lock();
        let took = html.as_deref().is_some_and(|h| g.doc.paste_html(h));
        if !took {
            g.doc.paste(&text);
        }
        g.view()
    }

    // ── formatting commands (mirror leaf-gpui's EditorCommand) ────────────────

    pub fn toggle_bold(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Strong);
        g.view()
    }

    pub fn toggle_italic(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Emph);
        g.view()
    }

    pub fn toggle_code(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Verbatim);
        g.view()
    }

    pub fn toggle_mark(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Mark);
        g.view()
    }

    pub fn toggle_underline(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Insert);
        g.view()
    }

    pub fn toggle_strike(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle(InlineKind::Delete);
        g.view()
    }

    pub fn set_paragraph(&self) -> DocView {
        let mut g = self.lock();
        g.doc.set_block(BlockKind::Paragraph);
        g.view()
    }

    /// Toggle the current block to a heading of `level` (1–6); toggling the
    /// active level off returns it to a paragraph, per core.
    pub fn set_heading(&self, level: u32) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_heading(level);
        g.view()
    }

    pub fn toggle_blockquote(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_blockquote();
        g.view()
    }

    pub fn toggle_list(&self, ordered: bool) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_list(ordered);
        g.view()
    }

    /// Tick or untick the task item at the caret. See
    /// [`leaf_core::Doc::toggle_task_checked`].
    pub fn toggle_task_checked(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_task_checked();
        g.view()
    }

    /// Tick or untick the task item covering `offset` — a tap on a rendered
    /// checkbox, which must not drag the caret across the document to get there.
    pub fn toggle_task_at(&self, offset: u64) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_task_at(offset as usize);
        g.view()
    }

    /// Give the list item at the caret a checkbox, or take its checkbox away.
    pub fn toggle_task_item(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_task_item();
        g.view()
    }

    /// Whether the item at the caret has a box and which way it faces — `None`
    /// for a plain list item or no item at all. Drives a toolbar's checked state.
    pub fn task_checked_at_caret(&self) -> Option<bool> {
        let mut g = self.lock();
        g.doc.task_checked_at_caret()
    }

    /// Which of the formatting commands above this document's format can
    /// actually spell — one flag per control, for building the toolbar.
    ///
    /// Read once when a document opens: the answer depends only on the format,
    /// so it cannot change under an edit. Every command refuses on its own
    /// regardless — the model is the authority, not the toolbar — so a frontend
    /// that ignores this stays correct, it just offers buttons whose only effect
    /// is a line in the status bar.
    ///
    /// Don't collapse it to one flag. An HTML document takes ⌘B, ⌘I and inline
    /// code (its marks are a tag pair) while refusing every heading, list, quote
    /// and link, and Markdown refuses the highlight djot spells — so a toolbar
    /// driven by [`Self::authorable`] alone would be wrong in both directions.
    pub fn capabilities(&self) -> Capabilities {
        self.lock().doc.capabilities().into()
    }

    /// Whether this document's format offers *any* door in — `false` only for a
    /// wholly parse-only one (XML), where an app may as well open the file
    /// read-only and hide the formatting section outright. For anything finer,
    /// including whether to dim an individual button, use [`Self::capabilities`].
    pub fn authorable(&self) -> bool {
        self.lock().doc.authorable()
    }

    // ── table editing ─────────────────────────────────────────────────────────

    /// Whether the caret is inside a table — for enabling the table controls.
    /// Pair it with [`Capabilities::table`]: the caret is genuinely inside an
    /// HTML `<table>`, and the grid controls still cannot edit one.
    pub fn caret_in_table(&self) -> bool {
        self.lock().doc.caret_in_table()
    }

    /// Insert an empty row below (`below`) or above the caret's row.
    pub fn table_insert_row(&self, below: bool) -> DocView {
        let mut g = self.lock();
        g.doc.table_insert_row(below);
        g.view()
    }

    /// Delete the caret's row (not the header or the last body row).
    pub fn table_delete_row(&self) -> DocView {
        let mut g = self.lock();
        g.doc.table_delete_row();
        g.view()
    }

    /// Insert an empty column right (`right`) or left of the caret's column.
    pub fn table_insert_column(&self, right: bool) -> DocView {
        let mut g = self.lock();
        g.doc.table_insert_column(right);
        g.view()
    }

    /// Delete the caret's column (unless it is the only one).
    pub fn table_delete_column(&self) -> DocView {
        let mut g = self.lock();
        g.doc.table_delete_column();
        g.view()
    }

    /// Set the caret's column to `alignment`.
    pub fn table_set_alignment(&self, alignment: TableAlignment) -> DocView {
        let mut g = self.lock();
        g.doc.table_set_alignment(alignment.into_core());
        g.view()
    }

    /// Move the caret's row one place down (`down`) or up.
    pub fn table_move_row(&self, down: bool) -> DocView {
        let mut g = self.lock();
        g.doc.table_move_row(down);
        g.view()
    }

    /// Move the caret's column one place right (`right`) or left.
    pub fn table_move_column(&self, right: bool) -> DocView {
        let mut g = self.lock();
        g.doc.table_move_column(right);
        g.view()
    }

    pub fn insert_link(&self, destination: String) -> DocView {
        let mut g = self.lock();
        g.doc.insert_link(&destination);
        g.view()
    }

    /// The destination of the link under the caret, if the caret is inside one —
    /// so a frontend can open it (⌘-click / "Open Link") or show it. `None` when the
    /// caret isn't on a link.
    pub fn link_destination_at_caret(&self) -> Option<String> {
        self.lock().doc.link_destination_at_caret()
    }

    /// The destination of the link at byte offset `off` —
    /// [`link_destination_at_caret`](Self::link_destination_at_caret) for a place
    /// the caret isn't.
    ///
    /// What a frontend drawing part of the document *outside* the document asks:
    /// a footnote's text in a popover has link runs in it, and this is how those
    /// runs learn where they point, since a `Run` carries how a span looks and
    /// not what it means.
    pub fn link_destination_at(&self, off: u32) -> Option<String> {
        self.lock().doc.link_destination_at(off as usize)
    }

    /// Where the locator `id` lands in this document — the `#v2` half of a
    /// `chapter.dj#v2`, resolved to the block it names. `None` when nothing here
    /// answers to it, which is a host's cue to open the document at its top
    /// rather than refuse to go.
    ///
    /// The query that gives a link finer granularity than the file. It reads an
    /// explicit `{#v1}`, a djot heading's minted id, or (for Markdown, which
    /// mints none) a heading's own words slugged — see [`leaf_core::Doc::locate`].
    ///
    /// Asked of *any* document, not only the open one: a host peeking at a
    /// citation builds a [`LeafDoc`] over the other file's bytes and asks this,
    /// which is what lets a hover show the verse instead of the filename.
    pub fn locate(&self, id: String) -> Option<LandingView> {
        self.lock().doc.locate(&id).map(LandingView::from)
    }

    /// Write a footnote at the caret — the toolbar's Footnote button. Both the
    /// `[^1]` and the definition it needs go in as one edit (one undo takes both
    /// back), the label is the lowest number the document has free, and the caret
    /// is left **in the empty note** ready to type it. Gate the button on
    /// [`Capabilities::footnote`]; see [`leaf_core::Doc::insert_footnote`].
    pub fn insert_footnote(&self) -> DocView {
        let mut g = self.lock();
        g.doc.insert_footnote();
        g.view()
    }

    /// The footnote reference under the caret, resolved to the note it names —
    /// so a frontend can show the note when a reader activates a `[1]`, instead
    /// of the nothing a reference click used to do. `None` when the caret isn't
    /// on a reference; see [`FootnoteView`] for the reference that resolved to
    /// no definition.
    pub fn footnote_at_caret(&self) -> Option<FootnoteView> {
        self.lock().doc.footnote_at_caret().map(FootnoteView::from)
    }

    /// The footnote reference at byte offset `off`, resolved to the note it
    /// names — [`footnote_at_caret`](Self::footnote_at_caret) for a place the
    /// caret isn't.
    ///
    /// This is what a hover asks: a pointer resting on a `[1]` wants the note's
    /// text in a popover, and moving the caret to find out would yank the reader
    /// out of wherever they were typing.
    pub fn footnote_at(&self, off: u32) -> Option<FootnoteView> {
        self.lock()
            .doc
            .footnote_at(off as usize)
            .map(FootnoteView::from)
    }

    /// The footnote definition the caret stands in, and where the reference that
    /// names it is — the return leg of [`footnote_at_caret`](Self::footnote_at_caret),
    /// so following a footnote is a round trip rather than a fall.
    ///
    /// `None` when the caret isn't in a definition, which is also how a frontend
    /// tells the two directions apart: the reference query answers up top, this
    /// one answers down in the notes, and never both at once.
    pub fn footnote_definition_at_caret(&self) -> Option<FootnoteDefView> {
        self.lock()
            .doc
            .footnote_definition_at_caret()
            .map(FootnoteDefView::from)
    }

    pub fn undo(&self) -> DocView {
        let mut g = self.lock();
        g.doc.undo();
        g.view()
    }

    pub fn redo(&self) -> DocView {
        let mut g = self.lock();
        g.doc.redo();
        g.view()
    }

    /// Switch between the rendered WYSIWYG surface and the raw source.
    pub fn toggle_view(&self) -> DocView {
        let mut g = self.lock();
        g.doc.toggle_view();
        g.view()
    }

    /// The current markup-exposure preference (see [`MarkupMode`]).
    pub fn markup_mode(&self) -> MarkupMode {
        MarkupMode::from_core(self.lock().doc.markup_mode())
    }

    /// Set the markup-exposure preference. Returns a fresh view so a frontend
    /// can repaint — and under `Full` it must, because the returned view is the
    /// first one showing the caret's line raw. Diaryx leaves it at the `None`
    /// default.
    pub fn set_markup_mode(&self, mode: MarkupMode) -> DocView {
        let mut g = self.lock();
        g.doc.set_markup_mode(mode.into_core());
        g.view()
    }

    /// The current soft-break flow preference (see [`LineFlow`]).
    pub fn line_flow(&self) -> LineFlow {
        LineFlow::from_core(self.lock().doc.line_flow())
    }

    /// Set the soft-break flow preference. Returns a fresh view so a frontend
    /// can repaint: like the markup-exposure preference this one changes rendering
    /// immediately, laying preserved soft breaks out as their own rows.
    pub fn set_line_flow(&self, mode: LineFlow) -> DocView {
        let mut g = self.lock();
        g.doc.set_line_flow(mode.into_core());
        g.view()
    }
}

// ── UITextInput support ──────────────────────────────────────────────────────
// A `UITextPosition` on the Swift side wraps a source byte offset; these are the
// offset↔geometry, stepping, and range-editing primitives the protocol needs.
// Queries never move the caret — they only read the (synced) visual map — so the
// system can probe positions freely while the model's selection stays put.
#[uniffi::export]
impl LeafDoc {
    /// The caret's source offset (the selection's moving end).
    pub fn caret_offset(&self) -> u32 {
        self.lock().doc.caret as u32
    }

    /// The selection's fixed end (equals the caret when there's no selection).
    pub fn anchor_offset(&self) -> u32 {
        let g = self.lock();
        g.doc.anchor.unwrap_or(g.doc.caret) as u32
    }

    /// The last caret stop in the document — `UITextInput.endOfDocument`.
    pub fn doc_end_offset(&self) -> u32 {
        let mut g = self.lock();
        g.sync();
        let end = g.doc.source.len();
        g.snap_stop(end) as u32
    }

    /// Snap an arbitrary offset to the nearest valid caret stop.
    pub fn snap_offset(&self, off: u32) -> u32 {
        let mut g = self.lock();
        g.sync();
        g.snap_stop(off as usize) as u32
    }

    /// Where a source offset sits on screen: its visual `(row, ch)`.
    pub fn pos_for_offset(&self, off: u32) -> RowCol {
        let mut g = self.lock();
        g.sync();
        let (row, col) = g.pos_of_offset(off as usize);
        let ch = col_to_utf16(&g.row_text(row), col);
        RowCol {
            row: row as u32,
            ch: ch as u32,
        }
    }

    /// The rows a source range covers, inclusive — for drawing a block away
    /// from where it sits (a footnote peek, a link peek, a landing flash).
    ///
    /// Ask this rather than mapping `start` and `end - 1` through
    /// [`Self::pos_for_offset`]. That pair reads correctly and is wrong: a
    /// block's last byte is often *hidden* — a note or a paragraph ending in a
    /// link ends inside the link's destination — and `pos_for_offset` snaps a
    /// hidden offset forward to the next visible glyph, which for a trailing
    /// one is on the next block's row. A peek slicing that span drew the block
    /// after it too. `pos_for_offset`'s snap is right for a caret and wrong for
    /// a span; this is the question spans should be asking.
    pub fn row_range_for(&self, start: u32, end: u32) -> RowRange {
        let mut g = self.lock();
        g.sync();
        let (first, last) = g.row_range_for(start as usize, end as usize);
        RowRange {
            first: first as u32,
            last: last as u32,
        }
    }

    /// The source offset at visual `(row, ch)` — the inverse of
    /// [`Self::pos_for_offset`], for hit-testing a point to a position.
    pub fn offset_for_pos(&self, row: u32, ch: u32) -> u32 {
        let mut g = self.lock();
        g.sync();
        let col = utf16_to_col(&g.row_text(row as usize), ch as usize);
        g.offset_of_col(row as usize, col) as u32
    }

    /// Move `off` by `delta` caret stops (negative = left) — `position(from:offset:)`.
    pub fn step_offset(&self, off: u32, delta: i32) -> u32 {
        let mut g = self.lock();
        g.sync();
        let mut o = g.snap_stop(off as usize);
        if delta >= 0 {
            for _ in 0..delta {
                match g.stop_after(o) {
                    Some(n) => o = n,
                    None => break,
                }
            }
        } else {
            for _ in 0..(-delta) {
                match g.stop_before(o) {
                    Some(p) => o = p,
                    None => break,
                }
            }
        }
        o as u32
    }

    /// The count of caret stops between two offsets (signed) — `offset(from:to:)`.
    pub fn distance_offset(&self, from: u32, to: u32) -> i32 {
        let mut g = self.lock();
        g.sync();
        let (from, to) = (from as usize, to as usize);
        let (mut a, b, sign) = if from <= to {
            (from, to, 1i32)
        } else {
            (to, from, -1i32)
        };
        a = g.snap_stop(a);
        let mut n = 0i32;
        while a < b {
            match g.stop_after(a) {
                Some(x) => {
                    a = x;
                    n += 1;
                }
                None => break,
            }
        }
        n * sign
    }

    /// The offset one navigable row up/down from `off`, keeping its column —
    /// `position(from:in: .up/.down)`. `None` at the top/bottom edge.
    pub fn vertical_offset(&self, off: u32, down: bool) -> Option<u32> {
        let mut g = self.lock();
        g.sync();
        let (row, col) = g.pos_of_offset(off as usize);
        let target = if down {
            g.nav_below(row)
        } else {
            g.nav_above(row)
        };
        target.map(|r| g.offset_of_col(r, col) as u32)
    }

    /// The visible text between two offsets — `text(in:)`. In the WYSIWYG
    /// view this is *not* the raw source slice: a hidden inline-mark
    /// delimiter (`**`, `` ` ``, `_`) contributes nothing, matching what
    /// `distance_offset`/`step_offset` already count in this same offset
    /// space — while a genuine block boundary the range spans (a paragraph
    /// gap, a table rule, …) contributes one inserted `'\n'` that
    /// `distance_offset`/`step_offset` do *not* count (a block boundary costs
    /// caret motion zero stops there, by design — see
    /// `the_caret_skips_the_gap_between_two_paragraphs` in `leaf-core`'s
    /// `doc.rs`). So the relationship is
    /// `text_in_range(a, b).chars().count() >= distance_offset(a, b)`, not
    /// strict equality: the two agree exactly when `(a, b)` spans no block
    /// boundary, and `text_in_range` is never shorter, only ever as long or
    /// longer, when it does. That inequality is still what
    /// `UITextInput`'s own word/line tokenizer needs (see
    /// [`leaf_core::wysiwyg::VisualMap::visible_text`] for why): it only reads
    /// this string to find a boundary and converts the result back to a
    /// position via `position(from:offset:)`, which walks stops — the
    /// inserted character is never hit as one, it only keeps the tokenizer
    /// from reading two paragraphs' last/first words as a single run of
    /// letters. The source view has nothing hidden to begin with, so there
    /// this is still exactly the raw slice.
    pub fn text_in_range(&self, from: u32, to: u32) -> String {
        let mut g = self.lock();
        g.sync();
        let len = g.doc.source.len();
        let (mut a, mut b) = ((from as usize).min(len), (to as usize).min(len));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        match g.doc.view {
            View::Wysiwyg => g.doc.vmap.visible_text(a, b),
            View::Source => {
                let s = &g.doc.source;
                while a > 0 && !s.is_char_boundary(a) {
                    a -= 1;
                }
                while b < s.len() && !s.is_char_boundary(b) {
                    b += 1;
                }
                s[a..b].to_string()
            }
        }
    }

    /// Set the selection to `[anchor, focus]` by source offsets — the setter behind
    /// `UITextInput.selectedTextRange` and handle dragging.
    pub fn set_selection_offsets(&self, anchor: u32, focus: u32) -> DocView {
        let mut g = self.lock();
        g.doc.place_caret(anchor as usize, false);
        if focus != anchor {
            g.doc.place_caret(focus as usize, true);
        }
        g.view()
    }

    /// Select the exact source range `[start, end)`, snapping neither end to a
    /// visible caret stop — for a host painting a range it already knows the
    /// bytes of (a search hit, an annotation) rather than hit-testing a touch.
    ///
    /// `set_selection_offsets` above is the *other* verb: it goes through
    /// `place_caret`, which snaps, and is what a drag handle wants. This one
    /// takes the range as given, so a selection over `**needle**`'s inner word
    /// is the word and not one byte short of it.
    pub fn select_range(&self, start: u32, end: u32) -> DocView {
        let mut g = self.lock();
        g.doc.select_range(start as usize, end as usize);
        g.view()
    }

    /// Replace the source range `[from, to]` with `text` — `replace(_:withText:)`.
    pub fn replace_range(&self, from: u32, to: u32, text: String) -> DocView {
        let mut g = self.lock();
        g.doc.place_caret(from as usize, false);
        if to != from {
            g.doc.place_caret(to as usize, true);
        }
        g.doc.insert(&text);
        g.view()
    }
}

impl LeafDoc {
    /// Acquire the guard, recovering from a poisoned lock: a panic in `leaf-core`
    /// under one call shouldn't wedge the whole document handle for the app.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The UTF-16 offset into `text` of display column `col`. Walks grapheme clusters
/// exactly as core measures columns ([`text_width`] per cluster), so a wide
/// cluster advances the column by its cells while the offset advances by its
/// UTF-16 length; the two coincide only on plain ASCII.
fn col_to_utf16(text: &str, col: usize) -> usize {
    let mut c = 0usize;
    let mut u = 0usize;
    for g in text.graphemes(true) {
        if c >= col {
            break;
        }
        c += text_width(g);
        u += g.chars().map(char::len_utf16).sum::<usize>();
    }
    u
}

/// The display column of the grapheme boundary at or before UTF-16 offset `off`
/// — the inverse of [`col_to_utf16`], turning a native click position back into
/// core's column. Core then clamps the column to a real caret stop.
fn utf16_to_col(text: &str, off: usize) -> usize {
    let mut c = 0usize;
    let mut u = 0usize;
    for g in text.graphemes(true) {
        if u >= off {
            break;
        }
        u += g.chars().map(char::len_utf16).sum::<usize>();
        c += text_width(g);
    }
    c
}

/// The renderer class id for a semantic role. Heading level is folded into the
/// id (`h1`…`h6`) so a single style rule per level applies.
fn role_name(r: Role) -> String {
    match r {
        Role::Body => "body".into(),
        Role::Heading(level) => format!("h{}", level.clamp(1, 6)),
        Role::Code => "code".into(),
        Role::Link => "link".into(),
        Role::Mark => "mark".into(),
        Role::ListMarker => "list".into(),
        Role::QuoteGutter => "quote".into(),
        Role::Rule => "rule".into(),
        Role::Image => "image".into(),
        Role::Delimiter => "delimiter".into(),
    }
}

/// The toolbar id for an inline mark — kept in sync with the Swift button ids.
fn mark_id(kind: InlineKind) -> &'static str {
    match kind {
        InlineKind::Strong => "bold",
        InlineKind::Emph => "italic",
        InlineKind::Verbatim => "code",
        InlineKind::Mark => "mark",
        InlineKind::Insert => "underline",
        InlineKind::Delete => "strike",
        InlineKind::Superscript => "superscript",
        InlineKind::Subscript => "subscript",
    }
}

/// The WYSIWYG rows: each visual row's glyphs coalesced into maximal runs of
/// identical `(style, selected)`. A glyph is selected when its source byte lies
/// in `[ss, se)`.
fn wysiwyg_rows(vmap: &VisualMap, ss: usize, se: usize, hls: &[leaf_core::Highlight]) -> Vec<Row> {
    vmap.rows
        .iter()
        .map(|vrow| {
            Row {
                runs: runs_of(&vrow.glyphs, ss, se, hls),
                decoration: vrow.decoration,
                code: vrow.code,
                code_lang: vrow.code_lang.clone(),
                directive: vrow.directive,
                directive_label: vrow.directive_label.clone(),
                // Straight off the row, not scanned out of its glyphs: an empty
                // heading has none to scan, and a renderer sizing the line by a
                // glyph's role drew `# ` at body height until it had text.
                heading: vrow.heading,
                boundary: vrow.boundary.map(|b| Boundary {
                    above: b.above.into(),
                    below: b.below.into(),
                }),
            }
        })
        .collect()
}

/// Coalesce `glyphs` into maximal runs of identical `(style, selected)` — the
/// shared body of a row's runs and a table cell's runs. A glyph is selected when
/// its source byte lies in `[ss, se)`.
/// Split a cell's flat glyphs into its visual lines at the in-cell break glyphs
/// (`\n`, from a `<br>`), each with the source range it spans. A line runs from
/// its first glyph's offset to the break that ends it (`cell_end` for the last);
/// an empty line — a leading/trailing break, or an empty cell — collapses to a
/// single caret home. The break glyphs themselves are dropped (they hold no
/// caret), exactly as the monospace picture drops them.
fn cell_lines(
    glyphs: &[leaf_core::Glyph],
    cell_start: usize,
    cell_end: usize,
    ss: usize,
    se: usize,
    hls: &[leaf_core::Highlight],
) -> Vec<TableCellLineView> {
    let mut lines = Vec::new();
    let mut seg: Vec<leaf_core::Glyph> = Vec::new();
    // The current line's start offset: the cell's for the first line, then the
    // first real glyph after each break (`None` until that glyph is seen).
    let mut line_start: Option<usize> = Some(cell_start);
    for g in glyphs {
        if g.ch == '\n' {
            let start = line_start.unwrap_or(g.src);
            lines.push(TableCellLineView {
                runs: runs_of(&seg, ss, se, hls),
                start: start as u32,
                end: g.src as u32,
            });
            seg.clear();
            line_start = None;
        } else {
            if line_start.is_none() {
                line_start = Some(g.src);
            }
            seg.push(g.clone());
        }
    }
    lines.push(TableCellLineView {
        runs: runs_of(&seg, ss, se, hls),
        start: line_start.unwrap_or(cell_end) as u32,
        end: cell_end as u32,
    });
    lines
}

fn runs_of(
    glyphs: &[leaf_core::Glyph],
    ss: usize,
    se: usize,
    hls: &[leaf_core::Highlight],
) -> Vec<Run> {
    // Which highlight (by index) covers a glyph — first by start when several
    // overlap, matching `Doc::highlight_at`. Part of the run key: a highlight
    // splits a run exactly the way the selection does, so its wash begins and
    // ends on its own bytes.
    let hl_of = |src: usize| hls.iter().position(|h| h.start <= src && src < h.end);
    let mut runs: Vec<Run> = Vec::new();
    let mut buf = String::new();
    // The style/selection/highlight key the run is accumulating, and the source
    // offset its first glyph came from — carried alongside rather than
    // re-derived, since a run's glyphs are contiguous but its *text* has no
    // offsets in it.
    let mut cur: Option<(LStyle, bool, Option<usize>, usize)> = None;
    for g in glyphs {
        let key = (g.style, g.src >= ss && g.src < se, hl_of(g.src));
        match cur {
            Some((style, sel, hl, _)) if (style, sel, hl) == key => buf.push(g.ch),
            _ => {
                if let Some((style, was_sel, hl, src)) = cur.take() {
                    runs.push(make_run(
                        std::mem::take(&mut buf),
                        style,
                        was_sel,
                        hl.map(|i| &hls[i]),
                        src,
                    ));
                }
                cur = Some((key.0, key.1, key.2, g.src));
                buf.push(g.ch);
            }
        }
    }
    if let Some((style, was_sel, hl, src)) = cur {
        runs.push(make_run(buf, style, was_sel, hl.map(|i| &hls[i]), src));
    }
    runs
}

/// The leaf directives of a WYSIWYG frame — each with the `rows` span its
/// placeholder occupies (to be painted over) and the name/attributes a frontend
/// resolves it by. The peer of [`wysiwyg_tables`] for a block that renders as a
/// thing rather than as text.
fn wysiwyg_directives(vmap: &VisualMap) -> Vec<DirectiveView> {
    vmap.directives
        .iter()
        .map(|d| DirectiveView {
            start_row: d.rows_span.start as u32,
            end_row: d.rows_span.end as u32,
            name: d.name.clone(),
            label: d.label.clone(),
            attrs: d
                .attrs
                .iter()
                .map(|(k, v)| DirectiveAttr {
                    key: k.clone(),
                    value: v.clone().unwrap_or_default(),
                })
                .collect(),
        })
        .collect()
}

/// The block media of a WYSIWYG frame — each with the `rows` span its
/// placeholder occupies (to be laid over) and what to build there. The peer of
/// [`wysiwyg_directives`], with each URL already resolved under `scheme`.
///
/// Resolving here rather than in Swift keeps the one piece of `<picture>` logic
/// core owns (`prefers-color-scheme` matching) in core. The `<source>` list
/// still crosses untouched, so a renderer can additionally pick by MIME — which
/// codecs AVFoundation has is not something core can know.
fn wysiwyg_media(vmap: &VisualMap, scheme: ColorScheme) -> Vec<MediaView> {
    vmap.media
        .iter()
        .map(|m| MediaView {
            start_row: m.rows_span.start as u32,
            end_row: m.rows_span.end as u32,
            kind: match m.kind {
                CoreMediaKind::Image => MediaKind::Image,
                CoreMediaKind::Video => MediaKind::Video,
                CoreMediaKind::Audio => MediaKind::Audio,
            },
            src: m.resolve(scheme).to_string(),
            poster: m.poster.clone(),
            alt: m.alt.clone(),
            sources: m
                .sources
                .iter()
                .map(|s| MediaSourceView {
                    media: s.media.clone(),
                    src: s.srcset.clone(),
                    mime: s.mime.clone(),
                })
                .collect(),
        })
        .collect()
}

/// The structural tables of a WYSIWYG frame — each with the `rows` span its
/// box-glyph picture occupies (to be skipped) and its grid of styled cells.
fn wysiwyg_tables(
    vmap: &VisualMap,
    ss: usize,
    se: usize,
    hls: &[leaf_core::Highlight],
) -> Vec<TableView> {
    vmap.tables
        .iter()
        .map(|t| TableView {
            start_row: t.rows_span.start as u32,
            end_row: t.rows_span.end as u32,
            grid: t
                .grid
                .iter()
                .map(|row| TableRowView {
                    head: row.head,
                    cells: row
                        .cells
                        .iter()
                        .map(|cell| TableCellView {
                            lines: cell_lines(&cell.glyphs, cell.start, cell.end, ss, se, hls),
                            align: align_name(cell.align),
                            start: cell.start as u32,
                            end: cell.end as u32,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

/// The wire name for a cell's column alignment.
fn align_name(a: Alignment) -> String {
    match a {
        Alignment::Left => "left",
        Alignment::Right => "right",
        Alignment::Center => "center",
        Alignment::Default => "default",
    }
    .to_string()
}

/// The source rows: the raw document split on `'\n'`, every line plain body text
/// with the `[ss, se)` selection carved out as its own run. Backs the source
/// view, whose caret rides raw byte offsets.
fn source_rows(source: &str, ss: usize, se: usize) -> Vec<Row> {
    let body = LStyle::default();
    let mut rows = Vec::new();
    let mut byte = 0usize;

    for raw in source.split('\n') {
        let start = byte;
        let end = start + raw.len();
        // Selection overlap with this line, in line-local byte coordinates.
        let a = ss.clamp(start, end) - start;
        let b = se.clamp(start, end) - start;

        // The source view's rows are split from raw text, so a run's offset is
        // simply where its slice starts — no glyphs to read one off.
        let mut runs = Vec::new();
        if a < b {
            if a > 0 {
                runs.push(make_run(raw[..a].to_string(), body, false, None, start));
            }
            runs.push(make_run(raw[a..b].to_string(), body, true, None, start + a));
            if b < raw.len() {
                runs.push(make_run(raw[b..].to_string(), body, false, None, start + b));
            }
        } else if !raw.is_empty() {
            runs.push(make_run(raw.to_string(), body, false, None, start));
        }

        rows.push(Row {
            runs,
            decoration: false,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            heading: None,  // source view is raw text — no resolved heading rows
            boundary: None, // …and no resolved block structure to divide
        });
        byte = end + 1; // skip the '\n' that `split` consumed
    }
    rows
}

/// Build a [`Run`] from an accumulated string and the core style it was drawn
/// with — the one place role and emphasis flags cross into the view shape.
fn make_run(
    text: String,
    style: LStyle,
    sel: bool,
    hl: Option<&leaf_core::Highlight>,
    src: usize,
) -> Run {
    Run {
        text,
        role: role_name(style.role),
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strike: style.strikethrough,
        sup: style.baseline == Baseline::Super,
        sub: style.baseline == Baseline::Sub,
        src: src as u32,
        sel,
        hl: hl.map(|h| h.id.clone()),
        hl_color: hl.and_then(|h| h.color.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Arc<LeafDoc> {
        LeafDoc::new(src.to_string(), "markdown".to_string()).unwrap()
    }

    #[test]
    fn a_footnote_definition_ending_the_file_is_itself_not_a_copy() {
        // No trailing newline: twig closes the last block on the virtual newline
        // it supplies at EOF, so the block's `span.end` is one past the source.
        // The definition and the `section` whose bytes contain it then both
        // overran, both keyed the block cache as *empty*, and the definition was
        // served the section's rows — this rendered the heading a second time.
        let src = "A claim[^1] worth checking.\n\n# A heading with a reference[^1] in it\n\n[^1]: The first note.\n[^note]: A note with a word for a label.";
        let d = LeafDoc::new(src.to_string(), "djot".to_string()).unwrap();
        let text: Vec<String> = d
            .view()
            .rows
            .iter()
            .map(|r| r.runs.iter().map(|x| x.text.as_str()).collect())
            .collect();
        assert_eq!(
            text.last().map(String::as_str),
            Some("[note] A note with a word for a label."),
            "the last definition should render itself: {text:?}"
        );
        assert_eq!(
            text.iter()
                .filter(|t| t.contains("A heading with a reference"))
                .count(),
            1,
            "the heading should render exactly once: {text:?}"
        );
    }

    #[test]
    fn an_empty_heading_crosses_the_boundary_carrying_its_level() {
        // What the toolbar's H1 leaves on a blank line: a heading with no text
        // yet. The renderer sizes a row by this field, so a `nil` here is a line
        // (and a caret) drawn at body height that jumps to heading height on the
        // first keystroke — the level can't be scanned out of the runs, because
        // an empty heading has none.
        let d = doc("body\n\n# \n");
        let v = d.view();
        let head = v.rows.last().expect("the heading's row");
        assert!(
            head.runs.iter().all(|r| r.text.is_empty()),
            "the `# ` marker is hidden"
        );
        assert_eq!(head.heading, Some(1));
        assert_eq!(
            v.rows[0].heading, None,
            "the paragraph above is not a heading"
        );
    }

    #[test]
    fn typing_into_a_heading_made_on_a_blank_line_keeps_the_caret_on_its_row() {
        // The reported bug at the boundary the Swift renderer reads: with a blank
        // line under it, the caret came back on a row two below the heading it
        // was actually in, and the view drew it there.
        let d = doc("one\n\ntwo\n\n\n\n");
        let _ = d.click(4, 0, false); // the first of the two blank lines
        let _ = d.set_heading(1);
        let mut v = d.view();
        for c in "title".chars() {
            v = d.insert(c.to_string());
        }
        assert_eq!(d.source(), "one\n\ntwo\n\n# title\n\n");
        assert_eq!(
            (v.caret_row, v.caret_ch),
            (4, 5),
            "the caret is on the heading's row"
        );
        assert_eq!(v.rows[4].heading, Some(1));
    }

    #[test]
    fn a_video_crosses_the_boundary_as_media_with_the_rows_to_lay_it_over() {
        // What the Swift renderer actually consumes: a row span to cover, a kind
        // to build a view from, and a URL to load. A frontend that skipped the
        // span would paint core's `🎬` placeholder underneath its own player.
        let d = doc("<video src=\"clip.mp4\" poster=\"still.png\" controls></video>\n");
        let v = d.view();
        assert_eq!(v.media.len(), 1);
        let m = &v.media[0];
        assert!(matches!(m.kind, MediaKind::Video));
        assert_eq!(m.src, "clip.mp4");
        assert_eq!(m.poster, "still.png");
        assert!(
            m.end_row > m.start_row,
            "the span must cover at least its label row"
        );
    }

    #[test]
    fn a_pictures_dark_source_resolves_by_appearance() {
        // The one piece of `<picture>` logic core owns, exercised across the
        // boundary: the same document resolves to a different URL depending on
        // what the host said its appearance was.
        let d = doc(
            "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"d.svg\">\
             <img src=\"l.svg\" alt=\"banner\"></picture>\n",
        );
        assert_eq!(d.view().media[0].src, "l.svg", "light by default");
        assert_eq!(d.set_dark_appearance(true).media[0].src, "d.svg");
        assert_eq!(d.set_dark_appearance(false).media[0].src, "l.svg");
    }

    #[test]
    fn tapping_below_a_trailing_picture_and_typing_keeps_it_a_picture() {
        // The whole gesture, across the boundary, in the order the Apple frontend
        // performs it: the layout clamps a point below the last row onto the
        // picture's row and asks for the position past its label glyphs; that
        // offset becomes the selection; then a character arrives. Before the two
        // halves of this fix, the offset was the stop *in front of* the picture
        // and the character dissolved it into a paragraph with an inline image —
        // the photo stopped being drawn, and nothing said so.
        let d = doc("hi\n\n![](p.png)\n");
        let v = d.set_unwrapped();
        let row = v.media[0].start_row;
        let label: u32 = v.rows[row as usize]
            .runs
            .iter()
            .map(|r| r.text.encode_utf16().count() as u32)
            .sum();

        let off = d.offset_for_pos(row, label);
        assert_eq!(
            off,
            "hi\n\n![](p.png)".len() as u32,
            "the stop past the picture"
        );

        d.set_selection_offsets(off, off);
        let after = d.insert("x".to_string());
        assert_eq!(d.source(), "hi\n\n![](p.png)\n\nx\n");
        assert_eq!(after.media.len(), 1, "still a picture, one paragraph up");
    }

    #[test]
    fn backspace_from_that_same_tap_takes_the_picture_whole() {
        // The other half of the same gesture, and the one that cost this project's
        // own test vault a photo: tap under the picture, press Backspace. That
        // offset is the stop past the markup, so a byte-step deleted the closing
        // paren and left the literal text `![](p.png` where a photo had been.
        let d = doc("hi\n\n![](p.png)\n");
        d.set_unwrapped();
        let off = "hi\n\n![](p.png)".len() as u32;
        d.set_selection_offsets(off, off);
        let after = d.backspace();
        assert_eq!(d.source(), "hi\n");
        assert_eq!(after.media.len(), 0, "gone as a picture, not as bytes");
        let undone = d.undo();
        assert_eq!(d.source(), "hi\n\n![](p.png)\n");
        assert_eq!(
            undone.media.len(),
            1,
            "and one undo brings the picture back"
        );
    }

    #[test]
    fn measured_heights_grow_the_reserved_span() {
        // The height loop: core reserves one row until the renderer measures the
        // real view and reports back, because core does no I/O and cannot know.
        let d = doc("![a cat](cat.png)\n");
        let before = &d.view().media[0];
        assert_eq!(
            before.end_row - before.start_row,
            1,
            "one row until measured"
        );

        let after = d.set_media_rows(vec![MediaHeight {
            destination: "cat.png".to_string(),
            rows: 6,
        }]);
        let m = &after.media[0];
        assert_eq!(
            m.end_row - m.start_row,
            6,
            "the span grew to what was measured"
        );
    }

    #[test]
    fn inserted_media_comes_straight_back_out_as_media() {
        // Round trip across the boundary, the pair that matters: what Swift asks
        // to insert, Swift sees on the very next frame.
        let d = doc("\n");
        let v = d.insert_media(
            MediaKind::Audio,
            "take.mp3".to_string(),
            "a take".to_string(),
        );
        assert_eq!(v.media.len(), 1);
        assert!(matches!(v.media[0].kind, MediaKind::Audio));
        assert_eq!(v.media[0].src, "take.mp3");
        assert_eq!(v.media[0].alt, "a take");
    }

    #[test]
    fn the_source_view_publishes_no_media() {
        // In the source view the `<video>` markup is the literal text the caret
        // is editing — laying a player over it would cover what's being typed.
        let d = doc("<video src=\"clip.mp4\" controls></video>\n");
        assert_eq!(d.view().media.len(), 1);
        assert!(
            d.toggle_view().media.is_empty(),
            "no placeholders in the source view"
        );
    }

    /// **A foreign caller's offset must never panic.** Every offset entering
    /// leaf comes from a UI toolkit that counts in its own units — UIKit hands
    /// back UTF-16 positions — so an offset landing mid-character is a normal
    /// thing to be handed, not a bug in the caller. Slicing on it aborts the
    /// process across the FFI boundary, where there is no unwinding to catch.
    ///
    /// Reproduces a real crash: `byte index 1236 is not a char boundary; it is
    /// inside '…'`.
    #[test]
    fn an_offset_inside_a_multibyte_char_does_not_panic() {
        let d = doc(
            "# April 02, 2026\n\nAn interesting thing AI said to me:\n\n> a person… who journals\n",
        );
        d.toggle_view(); // to the raw source view, where offsets index bytes directly
        let src = d.source();
        // The interior byte of the `…` — exactly the shape of the crash.
        let mid = src.find('…').expect("the ellipsis is in the fixture") + 1;
        assert!(
            !src.is_char_boundary(mid),
            "the fixture must be mid-character"
        );

        // Every entry point that takes a raw source offset.
        let _ = d.pos_for_offset(mid as u32);
        let _ = d.vertical_offset(mid as u32, true);
        let _ = d.vertical_offset(mid as u32, false);
        let _ = d.snap_offset(mid as u32);
        let _ = d.step_offset(mid as u32, 1);
        let _ = d.step_offset(mid as u32, -1);
        let _ = d.distance_offset(0, mid as u32);
        let _ = d.text_in_range(0, mid as u32);
        let _ = d.set_selection_offsets(mid as u32, mid as u32);
        // And the caret must not come to rest inside the character either — a
        // mid-character caret is a later panic waiting for the next edit.
        let _ = d.replace_range(mid as u32, mid as u32, "x".to_string());
        assert!(
            d.source().is_char_boundary(d.caret_offset() as usize),
            "the caret must sit on a character boundary"
        );
    }

    #[test]
    fn cell_lines_split_on_the_break_glyph_carrying_each_lines_source_range() {
        use leaf_core::Glyph;
        let g = |ch, src| Glyph {
            ch,
            style: LStyle::default(),
            src,
            stop: true,
        };
        // "a" at 10, a `<br>` at 11..15 (the break glyph), "b" at 15; cell 10..16.
        let glyphs = [g('a', 10), g('\n', 11), g('b', 15)];
        let lines = cell_lines(&glyphs, 10, 16, 0, 0, &[]);
        assert_eq!(lines.len(), 2, "one break makes two lines");
        assert_eq!(
            (lines[0].start, lines[0].end),
            (10, 11),
            "line 1 ends at the break"
        );
        assert_eq!(
            (lines[1].start, lines[1].end),
            (15, 16),
            "line 2 begins past it"
        );
        let text =
            |l: &TableCellLineView| l.runs.iter().map(|r| r.text.clone()).collect::<String>();
        assert_eq!(text(&lines[0]), "a");
        assert_eq!(text(&lines[1]), "b");

        // A trailing break leaves an empty last line homed at the cell's end.
        let trailing = [g('a', 10), g('\n', 11)];
        let lines = cell_lines(&trailing, 10, 15, 0, 0, &[]);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].runs.is_empty());
        assert_eq!((lines[1].start, lines[1].end), (15, 15));

        // No break: one line spanning the whole cell.
        let plain = [g('P', 10), g('e', 11)];
        let lines = cell_lines(&plain, 10, 12, 0, 0, &[]);
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0].start, lines[0].end), (10, 12));
    }

    fn row_text(v: &DocView, row: usize) -> String {
        v.rows[row].runs.iter().map(|r| r.text.clone()).collect()
    }

    #[test]
    fn unwrapped_collapses_a_paragraph_to_one_row() {
        let d = doc("one two three four five six seven eight\n");
        let wrapped = d.set_width(10);
        let unwrapped = d.set_unwrapped();
        assert!(
            unwrapped.rows.len() < wrapped.rows.len(),
            "a narrow column wrap splits the paragraph; unwrapped keeps it whole"
        );
        assert!(
            (0..unwrapped.rows.len()).any(|i| row_text(&unwrapped, i).contains("eight")),
            "the whole paragraph, including its last word, sits on a single unwrapped row"
        );
    }

    #[test]
    fn offsets_round_trip_when_unwrapped() {
        let d = doc("hello world\n");
        d.set_unwrapped();
        // offset -> (row, ch) -> offset is stable, so the pixel-wrapping frontend can
        // map between its visual lines and core's byte-offset caret model.
        let rc = d.pos_for_offset(6); // the 'w' of "world"
        assert_eq!(d.offset_for_pos(rc.row, rc.ch), 6);
    }

    #[test]
    fn set_unwrapped_is_idempotent() {
        let d = doc("a paragraph of some length here\n");
        let first = d.set_unwrapped();
        let second = d.set_unwrapped();
        assert_eq!(first.rows.len(), second.rows.len());
    }

    #[test]
    fn newline_on_last_list_item_before_a_blockquote_starts_a_new_item() {
        let src = "- one\n- two\n- three\n\n> quote\n";
        let d = doc(src);
        let off = (src.find("three").unwrap() + "three".len()) as u32; // end of "three" = 19
        d.set_selection_offsets(off, off);
        d.newline();
        let after = d.source();
        assert!(
            after.contains("- three\n- ") && after.contains("> quote"),
            "expected a new empty list item with the blockquote intact, got: {after:?}"
        );
    }

    #[test]
    fn enter_on_an_empty_line_adds_one_newline_and_one_backspace_undoes_it() {
        let d = doc("hello\n");
        d.set_selection_offsets(5, 5);
        d.newline(); // paragraph "hello" → a paragraph break, caret on the empty line
        let after_para = d.source();
        let caret_para = d.caret_offset();
        d.newline(); // Enter on the empty line
        assert_eq!(
            d.source().len(),
            after_para.len() + 1,
            "an empty-line Enter adds a single newline, not another paragraph break"
        );
        d.backspace(); // a single Backspace restores the previous state
        assert_eq!(d.source(), after_para);
        assert_eq!(d.caret_offset(), caret_para);
    }

    #[test]
    fn enter_in_a_nonempty_paragraph_still_opens_a_new_paragraph() {
        let d = doc("hello\n");
        d.set_selection_offsets(5, 5);
        let before = d.source().len();
        d.newline();
        assert_eq!(
            d.source().len(),
            before + 2,
            "a paragraph break is still \\n\\n"
        );
    }

    #[test]
    fn link_destination_at_caret_reads_the_caret_link() {
        let d = doc("see [t](https://x.dev) ok\n");
        d.set_selection_offsets(5, 5); // caret on the link text "t"
        assert_eq!(
            d.link_destination_at_caret().as_deref(),
            Some("https://x.dev")
        );
        d.set_selection_offsets(0, 0); // caret on plain text
        assert_eq!(d.link_destination_at_caret(), None);
    }

    #[test]
    fn the_frame_carries_the_caret_link_so_a_toolbar_can_light_and_seed_from_it() {
        // The reason it rides `DocView` rather than being asked for: stepping the
        // caret out of the link changes no other chrome fact on the frame, so a
        // toolbar that only redraws on a *changed* state would keep a stale light.
        let d = doc("see [t](https://x.dev) ok\n");
        d.set_selection_offsets(5, 5);
        let inside = d.view();
        assert_eq!(inside.link.as_deref(), Some("https://x.dev"));
        assert_eq!(inside.heading, None);
        assert!(inside.active.is_empty());

        d.set_selection_offsets(0, 0);
        let outside = d.view();
        assert_eq!(outside.link, None);
        // Nothing else the frame reports moved with it.
        assert_eq!(outside.heading, inside.heading);
        assert_eq!(outside.active, inside.active);
    }

    #[test]
    fn insert_footnote_crosses_and_leaves_the_caret_in_the_new_note() {
        // The button's round trip through the boundary: both halves written, and
        // a caret offset a host can type into without asking anything else.
        let d = doc("A claim and more.\n");
        d.set_selection_offsets(7, 7); // just past "A claim"
        d.insert_footnote();
        assert!(
            d.source().starts_with("A claim[^1] and more."),
            "{:?}",
            d.source()
        );
        assert!(d.source().contains("[^1]:"), "{:?}", d.source());

        let note = d.footnote_at(9).expect("the reference just written");
        assert_eq!(note.label, "1");
        assert_eq!(
            d.caret_offset(),
            note.offset.expect("an empty note is still a place")
        );
        // …and the way back out is the same one a reader uses.
        assert_eq!(
            d.footnote_definition_at_caret().expect("in the note").label,
            "1"
        );
    }

    #[test]
    fn capabilities_answer_for_footnotes_the_way_the_format_does() {
        assert!(
            doc("x\n").capabilities().footnote,
            "markdown spells the pair"
        );
        let html = LeafDoc::new("<p>x</p>\n".to_string(), "html".to_string()).unwrap();
        assert!(
            !html.capabilities().footnote,
            "html has no footnote of its own"
        );
    }

    #[test]
    fn footnote_at_caret_crosses_with_its_note_and_its_offset() {
        let d = doc("A claim[^1] and more.\n\n[^1]: the note\n");
        d.set_selection_offsets(9, 9); // caret on the reference's label
        let f = d
            .footnote_at_caret()
            .expect("the caret stands in a reference");
        assert_eq!(f.label, "1");
        assert_eq!(f.text.as_deref(), Some("the note"));
        // The note's first word — a byte the caret can actually rest on. The
        // definition's `[^1]:` marker is decoration with no stop of its own.
        assert_eq!(f.offset, Some(29));
        assert_eq!(f.end, Some(37));

        d.set_selection_offsets(0, 0); // caret on plain text
        assert!(d.footnote_at_caret().is_none());
    }

    #[test]
    fn footnote_at_crosses_for_an_offset_without_moving_the_caret() {
        // What a hover needs: the note under the pointer, and the caret left
        // exactly where the reader put it.
        let d = doc("A claim[^1] and more.\n\n[^1]: the note\n");
        d.set_selection_offsets(0, 0);
        let f = d.footnote_at(9).expect("offset 9 stands in the reference");
        assert_eq!(f.label, "1");
        assert_eq!(f.text.as_deref(), Some("the note"));
        assert_eq!(d.caret_offset(), 0, "asking must not move the caret");
        assert!(d.footnote_at(2).is_none(), "offset 2 is prose");
    }

    #[test]
    fn footnote_definition_at_caret_crosses_with_the_way_back() {
        let d = doc("A claim[^1] and more.\n\n[^1]: the note\n");
        d.set_selection_offsets(30, 30); // caret inside the note's body
        let f = d
            .footnote_definition_at_caret()
            .expect("the caret stands in a definition");
        assert_eq!(f.label, "1");
        assert_eq!(f.offset, Some(9), "the reference's label");

        // Disjoint from the reference query, which is what lets one gesture mean
        // "down" up top and "back up" down here.
        d.set_selection_offsets(9, 9);
        assert!(d.footnote_definition_at_caret().is_none());
        assert!(d.footnote_at_caret().is_some());
    }

    /// The contract a peek is built on: a note's offsets map to rows whose runs
    /// are the note *rendered* — emphasis as an italic run, `` `code` `` as a
    /// code run, a link as a link run — so a frontend draws it the way the
    /// document draws it instead of showing the reader raw asterisks.
    #[test]
    fn a_notes_offsets_map_to_its_rendered_rows() {
        let src = "Claim[^a].\n\n[^a]: see *emphasis* and `code` and [a link](https://x.dev).\n";
        let d = doc(src);
        let view = d.set_unwrapped();
        d.set_selection_offsets(6, 6); // the reference's label

        let f = d.footnote_at_caret().expect("a reference");
        let start = d.pos_for_offset(f.offset.expect("a note"));
        let end = d.pos_for_offset(f.end.expect("a note") - 1);
        assert_eq!(
            start.row, end.row,
            "a one-paragraph note is one unwrapped row"
        );

        let row = &view.rows[start.row as usize];
        let runs: Vec<(&str, &str, bool)> = row
            .runs
            .iter()
            .map(|r| (r.role.as_str(), r.text.as_str(), r.italic))
            .collect();
        assert!(runs.contains(&("body", "emphasis", true)), "got {runs:?}");
        assert!(
            runs.iter()
                .any(|(role, text, _)| *role == "code" && *text == "code"),
            "got {runs:?}"
        );
        assert!(
            runs.iter()
                .any(|(role, text, _)| *role == "link" && *text == "a link"),
            "got {runs:?}"
        );

        // The rendered row carries no markup characters at all — which is the
        // whole point, and what `text` (source bytes) deliberately still does.
        let rendered: String = row.runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            !rendered.contains('*') && !rendered.contains('`'),
            "got {rendered:?}"
        );
        assert!(
            f.text.as_deref().unwrap().contains('*'),
            "the source answer keeps them"
        );

        // `ch` is where the body starts within the row — past the `[a] ` marker,
        // so a frontend that wants the note without its label can slice there.
        assert_eq!(row.runs[0].role, "list");
        assert_eq!(start.ch as usize, row.runs[0].text.chars().count());

        // And each run says where it came from, which is how a link run drawn in
        // a popover learns where it points. `Run` otherwise says how a span
        // looks, never what it means.
        let link = row
            .runs
            .iter()
            .find(|r| r.role == "link")
            .expect("a link run");
        assert_eq!(
            d.link_destination_at(link.src).as_deref(),
            Some("https://x.dev"),
            "the run at {} is the link",
            link.src
        );
    }

    /// The peek bug, in the shape it was actually found in: three notes, each
    /// ending in a link, which is what a real citation block looks like.
    ///
    /// `a_notes_offsets_map_to_its_rendered_rows` above uses a note ending in a
    /// visible `.`, so its last byte has a row of its own and `end - 1` reads
    /// right. Take the full stop away — end the note *with* the link, as a
    /// citation does — and the last byte falls inside the hidden destination,
    /// where `pos_for_offset` snaps forward onto the next note's row. Hovering
    /// `[^2]` peeked notes 2 *and* 3.
    #[test]
    fn a_note_ending_in_a_link_covers_its_own_row_and_no_other() {
        let src = "A[^1] B[^2] C[^3].\n\n\
                   [^1]: https://en.wikipedia.org/wiki/Moravec%27s_paradox\n\n\
                   [^2]: [\"How to Get Startup Ideas,\" Nov 2012](https://www.paulgraham.com/startupideas.html)\n\n\
                   [^3]: [Alma 37:46](https://www.churchofjesuschrist.org/study/scriptures/bofm/alma/37?lang=eng&id=p46#p46)\n";
        let d = doc(src);
        let view = d.set_unwrapped();

        // The caret in the [^2] reference, exactly as a hover resolves it.
        let off2 = src.find("[^2] C").unwrap() as u32 + 2;
        d.set_selection_offsets(off2, off2);
        let f = d.footnote_at_caret().expect("a reference");
        let (start, end) = (f.offset.expect("a note"), f.end.expect("a note"));

        let span = d.row_range_for(start, end);
        assert_eq!(span.first, span.last, "one note is one unwrapped row");

        // And what it draws is note 2 alone — the assertion the popover failed.
        let drawn: String = view.rows[span.first as usize]
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        assert!(drawn.contains("How to Get Startup Ideas"), "got {drawn:?}");
        assert!(
            !drawn.contains("Alma"),
            "note 3 leaked into the peek: {drawn:?}"
        );

        // The old arithmetic, pinned as still wrong so nobody quietly restores
        // it: this is the failure `row_range_for` exists instead of.
        assert_ne!(
            d.pos_for_offset(end - 1).row,
            span.last,
            "the forward snap still leaves the note's row — that is the point",
        );

        // Note 1 is a bare autolink, whose visible text *is* its URL, so it was
        // never affected and must not change.
        let off1 = src.find("[^1] B").unwrap() as u32 + 2;
        d.set_selection_offsets(off1, off1);
        let f1 = d.footnote_at_caret().expect("a reference");
        let one = d.row_range_for(f1.offset.unwrap(), f1.end.unwrap());
        assert_eq!(one.first, one.last);
        assert_ne!(one.first, span.first, "and it is a different note");
    }

    /// A run's `src` is a byte offset core handed over, not something a frontend
    /// counted its way to — so multi-byte prose ahead of a link inside a note
    /// can't slide it.
    ///
    /// The offset is a *byte* offset while the run's text is characters and the
    /// row's columns are display cells; `src` is the only one of the three a
    /// frontend can use without converting between the other two.
    #[test]
    fn a_runs_source_offset_survives_multibyte_prose_ahead_of_it() {
        let src = "Claim[^a].\n\n[^a]: 日記 café [a link](https://x.dev).\n";
        let d = doc(src);
        let view = d.set_unwrapped();
        d.set_selection_offsets(6, 6);

        let f = d.footnote_at_caret().expect("a reference");
        let start = d.pos_for_offset(f.offset.expect("a note"));
        let row = &view.rows[start.row as usize];
        let link = row
            .runs
            .iter()
            .find(|r| r.role == "link")
            .expect("a link run");

        assert_eq!(
            d.link_destination_at(link.src).as_deref(),
            Some("https://x.dev")
        );
        assert_eq!(
            &src[link.src as usize..][.."a link".len()],
            "a link",
            "and it is a byte offset, not a character or column index"
        );
        // Which the character count is not: `日記 café ` is 9 characters and 13
        // bytes, so anything derived from the run text lands in the wrong place.
        let counted: usize = row
            .runs
            .iter()
            .take_while(|r| r.role != "link")
            .map(|r| r.text.chars().count())
            .sum();
        assert_ne!(counted, link.src as usize);
    }

    /// The round trip through the API a frontend actually calls — which places
    /// carets, and so snaps them to real stops. Offsets that named the `[^`
    /// markers passed every test that assigned the caret directly and still
    /// dumped the reader in the paragraph above the note.
    #[test]
    fn following_a_footnote_and_coming_back_lands_on_real_caret_stops() {
        let d = doc("A claim[^1] and more.\n\n[^1]: the note\n");
        d.set_selection_offsets(9, 9);

        let down = d
            .footnote_at_caret()
            .expect("a reference")
            .offset
            .expect("a note");
        d.set_selection_offsets(down, down);
        assert_eq!(
            d.caret_offset(),
            down,
            "the note is somewhere the caret fits"
        );

        let up = d
            .footnote_definition_at_caret()
            .expect("arrived inside the definition")
            .offset
            .expect("a reference to return to");
        d.set_selection_offsets(up, up);
        assert_eq!(d.caret_offset(), up, "and so is the reference");
        assert_eq!(
            d.footnote_at_caret().expect("back on the reference").label,
            "1"
        );
    }

    #[test]
    fn a_footnote_reference_crosses_the_ffi_raised() {
        // The whole point of the `sup` flag: without it a reference reaches
        // Swift as a run indistinguishable from a hyperlink's, which is why it
        // used to draw at body size.
        let d = doc("A claim[^1] and more.\n");
        let view = d.view();
        let runs: Vec<&Run> = view.rows.iter().flat_map(|r| &r.runs).collect();
        let chip = runs
            .iter()
            .find(|r| r.text.contains('1'))
            .expect("the reference's chip");
        assert!(chip.sup, "the reference should cross raised");
        assert!(!chip.sub);
        assert_eq!(
            chip.role, "link",
            "and still carrying the role every frontend paints"
        );
        // The prose it interrupts is a run of its own, on the normal baseline —
        // which is what proves the flag splits runs rather than bleeding.
        let prose = runs
            .iter()
            .find(|r| r.text.contains("claim"))
            .expect("the prose");
        assert!(!prose.sup && !prose.sub);
    }

    #[test]
    fn text_in_range_hides_delimiters_like_the_screen_does() {
        // "a **bold** c\n": 0:'a' 1:' ' 2:'*' 3:'*' 4:'b' 5:'o' 6:'l' 7:'d'
        // 8:'*' 9:'*' 10:' ' 11:'c' 12:'\n'. Bytes 8..10 are the closing `**`
        // — hidden, no glyph — and bytes 2..4 the opening `**`, likewise
        // hidden. `caret_steps_over_hidden_delimiters` in leaf-core already
        // pins that one Right from 7 (just past the 'd') lands on 10 (the
        // space before 'c'), skipping 8/9 entirely — so the *visible* text
        // transiting [7, 10) is exactly "d": the closing `**` contributes
        // nothing, matching what's drawn on screen.
        let d = doc("a **bold** c\n");
        assert_eq!(d.text_in_range(7, 10), "d");
        assert_eq!(
            d.text_in_range(7, 10).chars().count() as i32,
            d.distance_offset(7, 10),
            "text(in:).count() must equal offset(from:to:) — the UITextInput invariant this bug broke"
        );

        // Plain text with no hidden delimiter in range: unchanged, still the
        // raw slice, proving the fix doesn't regress the common case.
        assert_eq!(d.text_in_range(0, 1), "a");
        assert_eq!(d.text_in_range(11, 12), "c");
        assert_eq!(
            d.text_in_range(0, 1).chars().count() as i32,
            d.distance_offset(0, 1)
        );
    }

    #[test]
    fn text_in_range_matches_distance_offset_across_marked_up_and_plain_spans() {
        // The general invariant, straddling bold/italic/code spans and not:
        // for any pair of offsets, the visible text `text_in_range` returns
        // must have exactly as many `chars()` as `distance_offset` reports
        // stops between them — otherwise iOS's word tokenizer (which fetches
        // a text window, finds a boundary by indexing into *that string*, and
        // converts the index back to a position via `position(from:offset:)`)
        // resolves the boundary at the wrong offset.
        let d = doc("a **bold** _em_ and `code` here\n");
        let len = d.source().len() as u32;
        let mut pairs = Vec::new();
        let mut a = 0u32;
        while a < len {
            let mut b = a + 1;
            while b <= len {
                pairs.push((a, b));
                b += 3; // sample rather than an O(n^2) sweep
            }
            a += 1;
        }
        for (a, b) in pairs {
            let text = d.text_in_range(a, b);
            let dist = d.distance_offset(a, b).abs();
            assert_eq!(
                text.chars().count() as i32,
                dist,
                "text_in_range({a}, {b}) = {text:?} has {} chars, but distance_offset says {dist}",
                text.chars().count()
            );
        }
    }

    #[test]
    fn text_in_range_separates_paragraphs_so_words_dont_merge_across_the_gap() {
        // Regression: double-tapping the last word on a line immediately
        // followed by a paragraph break selected past the break into the
        // next paragraph — and kept compounding across further trivial
        // paragraphs in a row — because `text_in_range` returned the two
        // paragraphs' text with nothing between them: "hello" then "hello"
        // read back as one merged "hellohello" run of letters, no different
        // from the raw source concatenation, and iOS's word tokenizer duly
        // selected the whole run as a single word.
        let d = doc("hello\n\nhello\n\nhello\n");
        let src = d.source();
        assert_eq!(
            src.find("hello").unwrap(),
            0,
            "paragraph 1 at the very start"
        );
        let p2 = src[5..].find("hello").unwrap() + 5; // 7: paragraph 2's "hello"

        // A window straddling the tail of paragraph 1 ("lo") and the head of
        // paragraph 2 ("he").
        let text = d.text_in_range(3, p2 as u32 + 2);
        assert_ne!(
            text, "lohe",
            "the two paragraphs' words must not read as merged"
        );
        assert!(
            text.chars().any(|c| !c.is_alphanumeric()),
            "a non-letter must separate the two paragraphs' words: got {text:?}"
        );
        assert_eq!(
            text, "lo\nhe",
            "exactly one separator opens the second paragraph's head"
        );

        // A window that is nothing but the bare gap itself (no glyph on the
        // left, since it starts exactly at the end of paragraph 1's own last
        // row) must still carry the break — this is the case a naive
        // "insert a separator only between two real hits" fix undercounts,
        // since there is no earlier hit to anchor it to.
        let gap_only = d.text_in_range(5, p2 as u32);
        assert!(
            gap_only.chars().count() as i32 >= d.distance_offset(5, p2 as u32),
            "text_in_range must never be shorter than distance_offset: {gap_only:?}"
        );

        // The invariant the two existing tests above assert (strict
        // equality) no longer holds once the range spans a paragraph
        // boundary — see `text_in_range`'s doc comment — but it must never
        // *undercount* relative to `distance_offset`, which is what would let
        // a tokenizer's `position(from:offset:)` walk past where the text it
        // was handed actually put a boundary.
        for (a, b) in [(0u32, src.len() as u32), (3, p2 as u32 + 2), (5, p2 as u32)] {
            let text = d.text_in_range(a, b);
            let dist = d.distance_offset(a, b);
            assert!(
                text.chars().count() as i32 >= dist,
                "text_in_range({a}, {b}) = {text:?} ({} chars) is shorter than distance_offset {dist}",
                text.chars().count()
            );
        }

        // Caret motion itself is untouched by any of this: from the very end
        // of paragraph 1's row, a paragraph gap still costs exactly one
        // Right press to reach the start of paragraph 2 — matching
        // leaf-core's `the_caret_skips_the_gap_between_two_paragraphs`.
        assert_eq!(
            d.distance_offset(5, p2 as u32),
            1,
            "one Right crosses the whole gap"
        );
    }

    #[test]
    fn a_highlight_splits_runs_on_its_own_bytes_and_carries_its_id() {
        let d = doc("one two three\n");
        let view = d.set_highlights(vec![Highlight {
            start: 4,
            end: 7,
            id: "remark-1".into(),
            color: Some("#ffe066".into()),
            marker: Some("text.bubble".into()),
        }]);
        let row = &view.rows[0];
        let texts: Vec<(&str, Option<&str>)> = row
            .runs
            .iter()
            .map(|r| (r.text.as_str(), r.hl.as_deref()))
            .collect();
        assert_eq!(
            texts,
            [("one ", None), ("two", Some("remark-1")), (" three", None)],
            "the wash begins and ends exactly on the highlight's bytes"
        );
        assert_eq!(row.runs[1].hl_color.as_deref(), Some("#ffe066"));
        assert_eq!(d.highlight_at(5).as_deref(), Some("remark-1"));
        assert_eq!(
            d.highlights()
                .first()
                .and_then(|h| h.marker.clone())
                .as_deref(),
            Some("text.bubble"),
            "the marker rides back out for the frontend's margin pass"
        );
        assert_eq!(d.highlight_at(7), None, "end is exclusive");
        // A replace with nothing clears the wash.
        let view = d.set_highlights(Vec::new());
        assert!(view.rows[0].runs.iter().all(|r| r.hl.is_none()));
    }
}
