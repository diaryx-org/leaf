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

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use leaf_core::style::{Baseline, Role, Style as LStyle};
use leaf_core::wysiwyg::text_width;
use leaf_core::{
    Align as CoreAlign, Alignment, BlockKind, Capabilities as CoreCapabilities, ColorScheme, Doc,
    FaceTable as CoreFaceTable, FontFace as CoreFontFace, FontFamily as CoreFontFamily,
    FontSize as CoreFontSize, Format, Hundredths as CoreHundredths, InlineKind,
    LineFlow as CoreLineFlow, LineHeight as CoreLineHeight, LineSpacing as CoreLineSpacing,
    MarkColor as CoreMarkColor, MarkupMode as CoreMarkupMode, MediaKind as CoreMediaKind,
    SizeStep as CoreSizeStep, TextColor as CoreTextColor, TextCounts as CoreTextCounts, View,
    VisualMap,
};
use unicode_segmentation::UnicodeSegmentation;

// Linked, not used: see the dependency's note in Cargo.toml. The `as _` is
// what makes rustc treat the crate as referenced and carry its objects into
// the staticlib.
use resvg_uniffi as _;

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
    /// [`typeset_math`] could not read its TeX. `position` is a byte offset
    /// into the formula's text where the parser gave up, when it can say.
    #[error("math error: {message}")]
    Math {
        message: String,
        position: Option<u32>,
    },
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

/// How much writing there is — over the whole document, or over the
/// selection. The FFI shape of `leaf_core::TextCounts`; see
/// [`LeafDoc::counts`] for what is counted and what isn't.
#[derive(uniffi::Record)]
pub struct TextCounts {
    /// Words, by UAX#29 word segmentation: a segment holding at least one
    /// letter or digit, so `don't` is one and a lone dash is none.
    /// A hyphenated compound is two, which is what the algorithm says.
    pub words: u64,
    /// Characters as a reader counts them — grapheme clusters, spaces
    /// included. An emoji family and an accented letter are each one.
    pub characters: u64,
    /// The same, less every whitespace grapheme.
    pub characters_without_spaces: u64,
    /// Block-level containers holding at least one non-whitespace character:
    /// a paragraph, a heading, each list item, each paragraph inside a
    /// blockquote, a whole code block, a whole table.
    pub paragraphs: u64,
}

impl From<CoreTextCounts> for TextCounts {
    fn from(c: CoreTextCounts) -> Self {
        TextCounts {
            words: c.words as u64,
            characters: c.characters as u64,
            characters_without_spaces: c.characters_without_spaces as u64,
            paragraphs: c.paragraphs as u64,
        }
    }
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
    /// The colour the author named on a `mark` run — `"red"`, `"orange"`,
    /// `"yellow"`, `"green"`, `"blue"`, `"purple"`, `"brown"` — or absent for a
    /// plain `==highlight==` and for every other role.
    ///
    /// A *name*, unlike [`hl_color`](Self::hl_color)'s `#RRGGBB`, and that is
    /// the difference between the two: a host highlight's colour is the host's
    /// own choice and arrives as a value to paint, while this one is the
    /// document's word for it and the renderer picks the wash. It rides beside
    /// `role` rather than folding into it (`"mark-red"`) so a renderer that
    /// knows nothing about colours still draws the run as the highlight it is.
    pub mark_color: Option<String>,
    /// What a `code` run is to the language its fenced block is written in —
    /// `"punctuation"`, `"keyword"`, `"entity"`, `"support"`, `"constant"`,
    /// `"string"`, `"comment"`, `"invalid"` — or absent for a run the grammar
    /// left plain, for every run of a block in a language no grammar covers,
    /// for inline code, and for every other role.
    ///
    /// A class id like `role`, and beside it for the reason `mark_color` is: a
    /// renderer that knows nothing about tokens still draws the run as the code
    /// it is, and one that does keys a palette on the name.
    pub token: Option<String>,
    /// How large this run is set — one of CSS's seven keywords (`"xx-small"`,
    /// `"x-small"`, `"small"`, `"large"`, `"x-large"`, `"xx-large"`,
    /// `"xxx-large"`), or the exact size the author asked for (`"14pt"`,
    /// `"13.5pt"`) — and absent for the theme's own size, which is every run
    /// there was before the presentation vocabulary.
    ///
    /// A *token* rather than an enum, for [`mark_color`](Self::mark_color)'s
    /// reason and one more: a keyword says how much bigger and leaves how big
    /// to the theme (`leaf_core::SizeStep::scale` has CSS's own ratios for a
    /// renderer that wants a default ramp), while a `pt` size is exactly that
    /// many points of the sheet. A renderer reads its table first and parses
    /// the suffix when the table has no entry.
    pub size: Option<String>,
    /// The face this run is set in — one of CSS's four generics (`"serif"`,
    /// `"sans-serif"`, `"monospace"`, `"cursive"`), or a family the author
    /// named (`"Garamond"`) — and absent for the theme's body face.
    ///
    /// A generic is the theme's to name, so `serif` is whichever serif this
    /// platform's theme has and opens everywhere. A family name is resolved
    /// through the platform's font registry and falls back to the body face
    /// where it is not installed, which is the portability the author traded
    /// away knowingly.
    pub font: Option<String>,
    /// The run's *foreground* colour — one of the seven names
    /// [`mark_color`](Self::mark_color) carries, or six lowercase hex digits
    /// behind a `#` (`"#c03030"`) — and absent for the theme's text colour.
    ///
    /// A name is two inks, one per appearance, and the theme owns both; a
    /// triple is painted as written in the light appearance and in the dark
    /// one alike, which is what "exact" means.
    ///
    /// Not [`mark_color`](Self::mark_color), though they share a vocabulary on
    /// purpose: that is a highlight's *background* and reaches a run through its
    /// `mark` role, this is what the letters themselves are painted. A renderer
    /// with a red for a highlight has a red for text, and both should be that
    /// red.
    pub text_color: Option<String>,
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
    /// How this row's block is aligned across the measure — `"center"`,
    /// `"right"`, `"justify"` — and `None` for the theme's default, which is
    /// left. On every row the block emits.
    ///
    /// A *row* fact and not a run one for [`heading`](Self::heading)'s reason,
    /// and more sharply: alignment is a property of the line, not of the letters
    /// on it, so an empty paragraph the author has just centred carries it with
    /// no run to hang it on. A renderer sets its paragraph style's alignment
    /// from it. See [`leaf_core::VRow::align`].
    pub align: Option<String>,
    /// How far apart this row's block sets its lines, as a multiple of the
    /// theme's own line height — the menu's three (`"1.15"`, `"1.5"`, `"2"`)
    /// or any other positive decimal the author asked for (`"1.3"`) — and
    /// `None` for the theme's spacing, which is what `"1"` would mean and is
    /// why it is never written. On every row the block emits.
    ///
    /// The token *is* the ratio, so a renderer laying rows out in points
    /// multiplies its line height by it whether or not the menu has a row for
    /// it; one drawing a row per terminal line ignores it, the way it ignores a
    /// heading's size. See [`leaf_core::VRow::line_height`].
    pub line_height: Option<String>,
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
    /// A display formula on lines of its own — a `$$…$$` block.
    Math,
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
            K::Math => BlockClass::Math,
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

/// One formula standing as a picture: what to typeset and where its picture
/// goes. The peer of [`leaf_core::MathInfo`]. Two shapes:
///
/// - **Inline** (`inline == true`): one row, and on it exactly one run with
///   role `math` whose `src` equals this `src` — a single `∑` standing for the
///   whole formula. The renderer typesets the TeX at the run's font size
///   ([`typeset_math`]) and draws the picture in the run's place with the
///   text baseline through it at the picture's height: a run delegate on
///   Apple. The run is a caret stop at the formula's start; the one after it
///   is the next run's first character.
/// - **Block** (`inline == false`): the rows in `start_row..end_row` are the
///   placeholder, exactly a [`MediaView`]'s shape — the renderer **skips
///   them** and lays the typeset picture over them, centred on the measure.
///
/// A formula on the caret's line is not here: there it is its TeX, drawn as
/// `code` runs between `delimiter` runs, in every markup mode.
#[derive(uniffi::Record)]
pub struct MathView {
    /// The [`DocView::rows`] indices the formula occupies — its own row for an
    /// inline one, the placeholder and its fillers for a block.
    pub start_row: u32,
    pub end_row: u32,
    /// Whether this is an atom in a line of text, or a block of its own.
    pub inline: bool,
    /// The TeX between the delimiters, verbatim. What [`typeset_math`] takes.
    pub tex: String,
    /// Display style (limits above and below, full-height fractions) rather
    /// than text style — a `$$…$$`, inline or not.
    pub display: bool,
    /// The formula's source start: what the `math` run's `src` carries, and
    /// where a click on the picture lands the caret.
    pub src: u32,
}

/// A per-formula measured height, the way a renderer that reserves rows
/// (rather than laying pictures out in its own units) reports one back — the
/// input half of the loop [`LeafDoc::set_math_rows`] closes. The Swift views
/// lay a formula out in points and never need this.
#[derive(uniffi::Record)]
pub struct MathHeight {
    /// The formula's `tex` as [`MathView`] handed it over.
    pub tex: String,
    /// How many visual rows the picture needs.
    pub rows: u32,
}

/// A typeset formula: a standalone SVG document and where its baseline is.
/// The peer of `leaf_math::MathPicture`; see [`typeset_math`].
#[derive(uniffi::Record)]
pub struct MathPicture {
    /// A self-contained SVG — every glyph an outline, no font to find. Its
    /// `viewBox`, `width` and `height` are in pixels at the size it was
    /// typeset at, so drawn at its intrinsic size the glyphs land at that
    /// font size.
    pub svg: String,
    /// The picture's advance width, in em of the size it was typeset at.
    pub width: f64,
    /// How far it rises above its baseline, in em — the ascent a run
    /// delegate reports, so the text baseline passes through the picture
    /// here.
    pub height: f64,
    /// How far it reaches below its baseline, in em — the descent.
    pub depth: f64,
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
    /// Formulas standing as pictures — each inline atom and each display
    /// block — for a frontend that typesets and draws them in place of the
    /// `math` run or the placeholder rows. Empty in the source view, and
    /// empty of any formula on the caret's line, which is its TeX there.
    pub math: Vec<MathView>,
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
    /// Whether there is a step to undo, and one to redo — what a native Edit
    /// menu or an undo manager enables its items by. Both false on a read-only
    /// document. See [`leaf_core::Doc::can_undo`] for the bound this is.
    pub can_undo: bool,
    pub can_redo: bool,
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
    /// The colour of the highlight the caret stands in — which swatch a colour
    /// palette draws as the current one, and `None` both outside a highlight and
    /// inside an uncoloured one.
    ///
    /// It rides the frame for `link`'s reason, and more sharply: walking from a
    /// red highlight into a blue one changes no mark, no heading, no dirty flag
    /// and no link, so a palette asking for itself would never be told to move
    /// its checkmark.
    ///
    /// An enum rather than the name string [`Run::mark_color`] carries, because
    /// the two are different questions. A run's colour is a *rendering* hint that
    /// has to survive a name this build has never heard of (a newer twig's
    /// eighth colour draws as a plain highlight rather than not at all); this is
    /// the closed palette a control offers, and a name outside it is not a
    /// swatch anyone can press.
    pub mark_color: Option<MarkColor>,
}

/// The colour of a highlight — the closed palette [`LeafDoc::set_mark_color`]
/// writes and [`DocView::mark_color`] reports.
///
/// Obsidian's spelling, which is twig's: a circle emoji straight after the
/// opening `==`, so `==🔴 text==` is a highlight of `text` in [`MarkColor::Red`].
/// The emoji is *spelling* rather than content — it never appears in the text a
/// reader sees, and never in a [`Run`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum MarkColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Brown,
}

impl From<CoreMarkColor> for MarkColor {
    fn from(c: CoreMarkColor) -> Self {
        match c {
            CoreMarkColor::Red => MarkColor::Red,
            CoreMarkColor::Orange => MarkColor::Orange,
            CoreMarkColor::Yellow => MarkColor::Yellow,
            CoreMarkColor::Green => MarkColor::Green,
            CoreMarkColor::Blue => MarkColor::Blue,
            CoreMarkColor::Purple => MarkColor::Purple,
            CoreMarkColor::Brown => MarkColor::Brown,
        }
    }
}

impl From<MarkColor> for CoreMarkColor {
    fn from(c: MarkColor) -> Self {
        match c {
            MarkColor::Red => CoreMarkColor::Red,
            MarkColor::Orange => CoreMarkColor::Orange,
            MarkColor::Yellow => CoreMarkColor::Yellow,
            MarkColor::Green => CoreMarkColor::Green,
            MarkColor::Blue => CoreMarkColor::Blue,
            MarkColor::Purple => CoreMarkColor::Purple,
            MarkColor::Brown => CoreMarkColor::Brown,
        }
    }
}

/// How a block's lines are set across the measure — the closed vocabulary
/// [`LeafDoc::set_alignment`] writes and [`LeafDoc::alignment_at_caret`]
/// reports.
///
/// There is no `left`, because absence is left: the default alignment is the
/// theme's, and a document that agrees with it has no reason to say so. A
/// segmented control draws a fourth segment for it and calls `setAlignment(nil)`.
///
/// An enum rather than the name string [`Row::align`] carries, for
/// [`MarkColor`]'s reason: a row's alignment is a *rendering* fact that has to
/// survive a token this build has never heard of, while this is the closed set a
/// control offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum Align {
    Center,
    Right,
    Justify,
}

impl From<CoreAlign> for Align {
    fn from(a: CoreAlign) -> Self {
        match a {
            CoreAlign::Center => Align::Center,
            CoreAlign::Right => Align::Right,
            CoreAlign::Justify => Align::Justify,
        }
    }
}

impl From<Align> for CoreAlign {
    fn from(a: Align) -> Self {
        match a {
            Align::Center => CoreAlign::Center,
            Align::Right => CoreAlign::Right,
            Align::Justify => CoreAlign::Justify,
        }
    }
}

/// How far apart a block's lines are set, as a multiple of the theme's own line
/// height — the vocabulary [`LeafDoc::set_line_spacing`] writes.
///
/// Single spacing is absent for the reason `left` is absent from [`Align`]: it
/// is the theme's, and the menu entry for it is `setLineSpacing(nil)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum LineSpacing {
    /// `1.15` — the word processor's default "a little more air".
    OneFifteen,
    /// `1.5`.
    OneHalf,
    /// `2` — double spacing.
    Double,
}

impl From<CoreLineSpacing> for LineSpacing {
    fn from(l: CoreLineSpacing) -> Self {
        match l {
            CoreLineSpacing::OneFifteen => LineSpacing::OneFifteen,
            CoreLineSpacing::OneHalf => LineSpacing::OneHalf,
            CoreLineSpacing::Double => LineSpacing::Double,
        }
    }
}

impl From<LineSpacing> for CoreLineSpacing {
    fn from(l: LineSpacing) -> Self {
        match l {
            LineSpacing::OneFifteen => CoreLineSpacing::OneFifteen,
            LineSpacing::OneHalf => CoreLineSpacing::OneHalf,
            LineSpacing::Double => CoreLineSpacing::Double,
        }
    }
}

/// How large a run is set relative to the text around it — CSS's
/// `<absolute-size>` keywords with `medium` removed, because `medium` is
/// absence. What [`LeafDoc::set_font_size`] writes.
///
/// A *step*, never a measurement: a run set to `14pt` in a 12pt theme is a step
/// up and the same run under a 16pt theme is a step *down*, the author's intent
/// inverted by a change they never made. `Large` is a step up under every theme,
/// and `leaf_core::SizeStep::scale` has CSS's own ratio for each if the theme
/// wants a default ramp.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum SizeStep {
    XxSmall,
    XSmall,
    Small,
    Large,
    XLarge,
    XxLarge,
    XxxLarge,
}

impl From<CoreSizeStep> for SizeStep {
    fn from(s: CoreSizeStep) -> Self {
        match s {
            CoreSizeStep::XxSmall => SizeStep::XxSmall,
            CoreSizeStep::XSmall => SizeStep::XSmall,
            CoreSizeStep::Small => SizeStep::Small,
            CoreSizeStep::Large => SizeStep::Large,
            CoreSizeStep::XLarge => SizeStep::XLarge,
            CoreSizeStep::XxLarge => SizeStep::XxLarge,
            CoreSizeStep::XxxLarge => SizeStep::XxxLarge,
        }
    }
}

impl From<SizeStep> for CoreSizeStep {
    fn from(s: SizeStep) -> Self {
        match s {
            SizeStep::XxSmall => CoreSizeStep::XxSmall,
            SizeStep::XSmall => CoreSizeStep::XSmall,
            SizeStep::Small => CoreSizeStep::Small,
            SizeStep::Large => CoreSizeStep::Large,
            SizeStep::XLarge => CoreSizeStep::XLarge,
            SizeStep::XxLarge => CoreSizeStep::XxLarge,
            SizeStep::XxxLarge => CoreSizeStep::XxxLarge,
        }
    }
}

/// The face a run is set in — CSS's generic families, less `fantasy` and
/// `system-ui`, neither of which an author asks for. What
/// [`LeafDoc::set_font_family`] writes.
///
/// A generic, never a font name, for [`SizeStep`]'s reason: a document naming
/// `Georgia` renders in the fallback everywhere Georgia is not installed. The
/// theme names the concrete face for each — `Serif` is whichever serif this
/// platform's theme has, and `Monospace` is the face inline code already uses.
/// The theme's own body face is absent because it is absence; the menu entry for
/// it is `setFontFamily(nil)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FontFamily {
    Serif,
    SansSerif,
    Monospace,
    Cursive,
}

impl From<CoreFontFamily> for FontFamily {
    fn from(f: CoreFontFamily) -> Self {
        match f {
            CoreFontFamily::Serif => FontFamily::Serif,
            CoreFontFamily::SansSerif => FontFamily::SansSerif,
            CoreFontFamily::Monospace => FontFamily::Monospace,
            CoreFontFamily::Cursive => FontFamily::Cursive,
        }
    }
}

impl From<FontFamily> for CoreFontFamily {
    fn from(f: FontFamily) -> Self {
        match f {
            FontFamily::Serif => CoreFontFamily::Serif,
            FontFamily::SansSerif => CoreFontFamily::SansSerif,
            FontFamily::Monospace => CoreFontFamily::Monospace,
            FontFamily::Cursive => CoreFontFamily::Cursive,
        }
    }
}

// ── the exact forms: a value where a name will not do ───────────────────────
//
// Each of the four closed enums above gets an *open* one beside it, which is a
// Swift enum with associated values: `.step(.large)` or `.points(14)`,
// `.generic(.serif)` or `.named("Garamond")`, `.named(.red)` or `.rgb(…)`,
// `.step(.oneHalf)` or `.ratio(1.3)`. The four gestures and four queries take
// and answer these; the closed enums stay because they are what a menu offers
// *first* — a name is portable under every theme, and the value under the
// divider is the author's to take knowingly. See
// `docs/proposals/exact-presentation-values.md`.
//
// **A value this vocabulary cannot carry is refused, and the gesture writes
// nothing at all.** A size of `0pt` or `700pt`, a ratio of `0`, a face named
// `"   "` — an author who typed one of those asked for something the document
// cannot hold, and the answer to that is to leave the document as it is. It is
// emphatically *not* to clear the key: the size that was already on the run is
// not the author's mistake, and a field that refuses by throwing away what was
// there is a field that punishes a typo. Validate before calling anyway — the
// range is 0.01 to 655.35 — because from here a refusal is silent.
//
// **One value does mean absence**, and it is the one core already states: a
// line height of `1` is single spacing, which is the theme's own and has no
// token, so `.ratio(1)` *clears* the key. That is the author asking for the
// default, not failing to ask for anything. [`Meant`] is the three answers
// written down once.

/// What a presentation value a caller handed in comes to — the three answers
/// the note above states, written down once.
///
/// A gesture asks for this and writes `Some(value)`, writes `None`, or writes
/// nothing at all. The middle one is a *clearing* the author asked for and the
/// last is a refusal; from the other side of the binding they look alike, which
/// is why a field that offers a number validates before it calls.
enum Meant<T> {
    /// A value core carries. The gesture writes it.
    Value(T),
    /// A value that *means* the theme's own. The gesture clears the key — only
    /// a line height of 1 is one of these.
    Absence,
    /// A value outside what the vocabulary carries. The gesture writes nothing,
    /// and what the run already said stands.
    Refused,
}

impl<T> Meant<T> {
    /// A core constructor's `Option` read as a refusal — the shape three of the
    /// four conversions below have, since only a spacing has an absence.
    fn of(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Value(value),
            None => Self::Refused,
        }
    }
}

/// What a gesture handed `value` writes: `Some(Some(v))` for a value,
/// `Some(None)` for the clearing both a `nil` argument and an absence mean, and
/// `None` for a value the vocabulary refuses — which the gesture spells by not
/// calling core at all.
fn written<T, C>(value: Option<T>, into_core: impl FnOnce(T) -> Meant<C>) -> Option<Option<C>> {
    match value.map(into_core) {
        None | Some(Meant::Absence) => Some(None),
        Some(Meant::Value(value)) => Some(Some(value)),
        Some(Meant::Refused) => None,
    }
}

/// How large a run is set: a [`SizeStep`] relative to the text around it, or
/// the point size the author asked for. What [`LeafDoc::set_font_size`] writes
/// and [`LeafDoc::font_size_at_caret`] answers.
///
/// The step is what a menu offers first and what a document should say where a
/// name will do — it reads as a step up under every theme. The point size is
/// what the author typed and is all it is: 14 points of the sheet on paper, and
/// 14 points before the zoom on screen. A heading set to an exact size is that
/// size and not its ramp scaled.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum FontSize {
    Step(SizeStep),
    /// Points. 0.01 to 655.35; anything else is refused — see the note above.
    Points(f64),
}

impl FontSize {
    /// The core size this names — [`Meant::Refused`] for a number of points
    /// core cannot carry, which leaves the run's size alone.
    fn into_core(self) -> Meant<CoreFontSize> {
        match self {
            FontSize::Step(step) => Meant::Value(CoreFontSize::Step(step.into())),
            FontSize::Points(points) => Meant::of(CoreFontSize::points(points as f32)),
        }
    }
}

impl From<CoreFontSize> for FontSize {
    fn from(s: CoreFontSize) -> Self {
        match s {
            CoreFontSize::Step(step) => FontSize::Step(step.into()),
            // Divided in `f64` rather than widened from the `f32` `as_f32`
            // answers: 1.3 as an `f32` widens to 1.2999999523, and a caller
            // that passed 1.3 in has to get 1.3 back or no menu row ticks.
            CoreFontSize::Points(pt) => FontSize::Points(f64::from(pt.hundredths()) / 100.0),
        }
    }
}

/// How far apart a block's lines are set: a [`LineSpacing`] from the menu's
/// three, or the ratio the author asked for. [`FontSize`]'s peer one property
/// along, and with no unit at all — a line height is a multiple.
///
/// A ratio that spells one of the three names *is* that name, so
/// `.ratio(1.5)` comes back as `.step(.oneHalf)` and a menu has a row to tick.
/// A ratio of 1 is single spacing, which is absence: it clears the key.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum LineHeight {
    Step(LineSpacing),
    /// A multiple of the theme's line height. 1 is absence, and clears; a
    /// number outside 0.01 to 655.35 is refused — see the note above.
    Ratio(f64),
}

impl LineHeight {
    /// The core spacing this names — the one [`into_core`](FontSize::into_core)
    /// here whose answer can be [`Meant::Absence`], because a ratio of 1 is
    /// single spacing and single spacing has no token.
    fn into_core(self) -> Meant<CoreLineHeight> {
        match self {
            LineHeight::Step(step) => Meant::Value(CoreLineHeight::Step(step.into())),
            // A number the vocabulary carries at all is asked first, because
            // `ratio` answers `None` both for a 1 — which *means* the theme's
            // own — and for a 0 or a NaN, which mean nothing at all, and the
            // two have opposite answers.
            LineHeight::Ratio(ratio) => match CoreHundredths::from_f32(ratio as f32) {
                None => Meant::Refused,
                Some(_) => match CoreLineHeight::ratio(ratio as f32) {
                    Some(height) => Meant::Value(height),
                    None => Meant::Absence,
                },
            },
        }
    }
}

impl From<CoreLineHeight> for LineHeight {
    fn from(l: CoreLineHeight) -> Self {
        match l {
            CoreLineHeight::Step(step) => LineHeight::Step(step.into()),
            // In `f64` throughout, for [`FontSize`]'s reason.
            CoreLineHeight::Ratio(r) => LineHeight::Ratio(f64::from(r.hundredths()) / 100.0),
        }
    }
}

/// A run's *foreground* colour: one of the seven [`MarkColor`] names, or the
/// RGB triple the author asked for.
///
/// A name is two inks, one per appearance, and the theme owns both. A triple is
/// painted as written in the light appearance and in the dark one alike — that
/// is what "exact" means, and the theme does not soften it.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum TextColor {
    Named(MarkColor),
    Rgb { r: u8, g: u8, b: u8 },
}

impl TextColor {
    /// The core colour this names. A plain conversion and not a [`Meant`],
    /// because every triple of bytes is a colour: this is the one of the four
    /// with nothing to refuse.
    fn into_core(self) -> CoreTextColor {
        match self {
            TextColor::Named(color) => CoreTextColor::Named(color.into()),
            TextColor::Rgb { r, g, b } => CoreTextColor::Rgb { r, g, b },
        }
    }
}

impl From<CoreTextColor> for TextColor {
    fn from(c: CoreTextColor) -> Self {
        match c {
            CoreTextColor::Named(color) => TextColor::Named(color.into()),
            CoreTextColor::Rgb { r, g, b } => TextColor::Rgb { r, g, b },
        }
    }
}

/// The face a run is set in: one of CSS's four generics, or the family the
/// author named.
///
/// A generic opens on every machine and a family name does not, which is the
/// trade [`FontFamily`]'s note states once. A named family is resolved through
/// the platform's font registry and falls back to the theme's body face where
/// it is not installed.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum FontFace {
    Generic(FontFamily),
    /// A family name, as the font panel spells it. Trimmed on the way in, and
    /// one that spells a generic (`"Serif"`) is read as that generic.
    Named(String),
}

impl FontFace {
    /// The core face this names — [`Meant::Refused`] for a name that names
    /// nothing, which leaves the run's face alone.
    fn into_core(self) -> Meant<CoreFontFace> {
        match self {
            FontFace::Generic(family) => Meant::Value(CoreFontFace::Generic(family.into())),
            // Through the parser rather than straight into `Named`, so a name
            // gets the trim and the generic-keyword reading a document's own
            // `data-font` gets, and an empty one names nothing.
            FontFace::Named(name) => Meant::of(CoreFontFace::from_attr(&name)),
        }
    }
}

impl From<CoreFontFace> for FontFace {
    fn from(f: CoreFontFace) -> Self {
        match f {
            CoreFontFace::Generic(family) => FontFace::Generic(family.into()),
            CoreFontFace::Named(name) => FontFace::Named(name),
        }
    }
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
    /// [`LeafDoc::set_mark_color`] — the highlight *palette*, which Markdown
    /// spells and djot does not, though both spell the highlight itself. Gate
    /// the swatches on this *and* on [`LeafDoc::caret_in_mark`], the way the grid
    /// controls take `table` and `caret_in_table`: one is a fact about the
    /// format, the other about where the caret is standing.
    pub mark_color: bool,
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
    /// The alignment control — [`LeafDoc::set_alignment`]. Every format leaf
    /// opens but XML spells a block's attributes.
    pub alignment: bool,
    /// The line-spacing menu — [`LeafDoc::set_line_spacing`]. The same gesture
    /// as [`alignment`](Self::alignment) and so the same answer, and its own
    /// flag because a toolbar dims controls one at a time.
    pub line_spacing: bool,
    /// The size menu — [`LeafDoc::set_font_size`]. **Narrower than the block
    /// pair**: it wraps a selection in an attributed span, which AsciiDoc has no
    /// slot for, so this is `false` there while [`alignment`](Self::alignment) is
    /// `true`. The block-level form of the same property — the caret in a
    /// paragraph, nothing selected — still works, which is why the flag
    /// describes the control rather than the caret.
    pub font_size: bool,
    /// The face menu — [`LeafDoc::set_font_family`]. A span, as
    /// [`font_size`](Self::font_size) is.
    pub font_family: bool,
    /// The text-colour swatches — [`LeafDoc::set_text_color`]. A span again, and
    /// not to be confused with [`mark_color`](Self::mark_color): that is a
    /// highlight's background and rides the `mark` node, this is a run's
    /// foreground and rides an attributed span.
    pub text_color: bool,
    /// The page-break button — [`LeafDoc::insert_page_break`]. Markdown and djot
    /// and no others, though twig spells the gesture in HTML and AsciiDoc too:
    /// the flag describes what leaf can *show*, and the walker draws neither of
    /// those spellings yet.
    pub page_break: bool,
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
            mark_color: c.mark_color,
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
            alignment: c.alignment,
            line_spacing: c.line_spacing,
            font_size: c.font_size,
            font_family: c.font_family,
            text_color: c.text_color,
            page_break: c.page_break,
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

    /// [`snap_stop`](Self::snap_stop) for the walks that pair stops with
    /// characters — `step_offset` and `distance_offset`, which a system text
    /// input counts against the text it was shown. The caret's home at the
    /// end of a hidden mark (`VisualMap::mark_ends`) has no character of its
    /// own, so those walks start from the glyph stop drawn at the same spot.
    fn snap_glyph_stop(&self, off: usize) -> usize {
        match self.doc.view {
            View::Wysiwyg => self
                .doc
                .vmap
                .snap_to_glyph_stop(off.min(self.doc.source.len())),
            View::Source => self.snap_stop(off),
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

        // Formulas, likewise: pictures stand in only in the rich view.
        let math = match self.doc.view {
            View::Wysiwyg => wysiwyg_math(&self.doc.vmap),
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
        let mark_color = self.doc.mark_color_at_caret().map(MarkColor::from);

        DocView {
            rows,
            tables,
            directives,
            media,
            math,
            caret_row: caret_row as u32,
            caret_col: caret_col as u32,
            caret_ch: caret_ch as u32,
            caret_src: self.doc.caret.min(self.doc.source.len()) as u32,
            has_selection,
            anchor_row: anchor_row as u32,
            anchor_ch: anchor_ch as u32,
            dirty: self.doc.dirty,
            can_undo: self.doc.can_undo(),
            can_redo: self.doc.can_redo(),
            view: self.doc.view_name().to_string(),
            heading,
            active,
            link,
            mark_color,
        }
    }
}

/// Typeset `tex` — the text between a formula's delimiters, as a [`MathView`]
/// hands it over — to a picture. `display` is the view's `display`; `size` is
/// the font size in points the formula is set at (an inline formula takes the
/// run's, a block the body's); `r`, `g`, `b`, `a` are the ink, as bytes. A
/// theme change is a re-render with a new colour.
///
/// Pure layout over fonts embedded in the binary — no I/O, fast enough to
/// call from a layout pass — so a renderer caches by `(tex, display, size,
/// colour)` and nothing more. TeX the typesetter cannot read is a
/// [`LeafError::Math`]; the renderer shows the revealed source in its place.
#[uniffi::export]
pub fn typeset_math(
    tex: String,
    display: bool,
    size: f64,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) -> Result<MathPicture, LeafError> {
    let p = leaf_math::typeset(&tex, display, size, [r, g, b, a]).map_err(|e| LeafError::Math {
        message: e.message,
        position: e.position.map(|p| p as u32),
    })?;
    Ok(MathPicture {
        svg: p.svg,
        width: p.width,
        height: p.height,
        depth: p.depth,
    })
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

    /// Report how many visual rows each display formula needs, keyed by its
    /// TeX as [`MathView`] handed it over — [`set_media_rows`]'s peer for a
    /// renderer that reserves rows. One that lays a formula out in its own
    /// units, as the Swift views do, never calls this.
    ///
    /// [`set_media_rows`]: Self::set_media_rows
    pub fn set_math_rows(&self, heights: Vec<MathHeight>) -> DocView {
        let mut g = self.lock();
        g.doc.set_math_rows(
            heights
                .into_iter()
                .map(|h| (h.tex, h.rows.max(1) as usize))
                .collect(),
        );
        g.view()
    }

    /// Say whether the renderer can paint a picture *inside* a line of text.
    /// When it can, an inline formula arrives as one `math` run and a
    /// [`MathView`] with `inline` set, for the renderer to draw its typeset
    /// picture over; when it cannot, as the code-styled TeX it always was.
    /// Off until called, so a host that has not caught up sees what it saw.
    pub fn set_inline_pictures(&self, on: bool) -> DocView {
        let mut g = self.lock();
        g.doc.set_inline_pictures(on);
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

    /// Words, characters, and paragraphs over the whole document — the numbers
    /// a status bar or an inspector puts next to a piece of writing.
    ///
    /// Counted over the text a reader sees rather than the markup that spells
    /// it: `**bold**` is one word and four characters, a link is its label and
    /// not its destination, a picture counts nothing, and frontmatter is not
    /// writing. The same in both views — the count reads neither the view nor
    /// the map the host last built. See `leaf_core::Doc::counts`.
    ///
    /// It is O(document) and not free (about 4 ms on a 45 KB file), so ask
    /// when the typing settles rather than on every keystroke; there is no
    /// `DocView` in it, because nothing about the document changes by being
    /// counted.
    pub fn counts(&self) -> TextCounts {
        self.lock().doc.counts().into()
    }

    /// The same statistics over the selection alone — `None` when nothing is
    /// selected. See `leaf_core::Doc::selection_counts`.
    pub fn selection_counts(&self) -> Option<TextCounts> {
        self.lock().doc.selection_counts().map(Into::into)
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

    /// Whether the caret stands in a highlight — what a colour palette enables
    /// itself by, since a colour is a property of a highlight that already
    /// exists. The caret-side half of [`Capabilities::mark_color`].
    pub fn caret_in_mark(&self) -> bool {
        self.lock().doc.caret_in_mark()
    }

    /// Colour the highlight at the caret, or clear its colour with `None`.
    ///
    /// Markdown only — `==🔴 text==` is its spelling and djot has none — and
    /// only where there is a highlight to colour: this does not make one, so a
    /// coloured highlight from bare text is [`LeafDoc::toggle_mark`] and then
    /// this, which is the order the button and its palette already sit in. Both
    /// refusals leave the document alone and say so in the status line.
    pub fn set_mark_color(&self, color: Option<MarkColor>) -> DocView {
        let mut g = self.lock();
        g.doc.set_mark_color(color.map(CoreMarkColor::from));
        g.view()
    }

    /// One press of a colour swatch: colour the highlight at the caret, or —
    /// over a selection that isn't highlighted yet — highlight it and colour it,
    /// as **one** undo step.
    ///
    /// [`LeafDoc::set_mark_color`] is the exact gesture; this is the compound a
    /// toolbar presses, and it lives in core so that every frontend answers
    /// "what does a swatch mean over plain text" the same way. `None` clears the
    /// colour, and over an unhighlighted selection means simply "highlight
    /// this". A bare caret in no highlight is left alone.
    pub fn highlight(&self, color: Option<MarkColor>) -> DocView {
        let mut g = self.lock();
        g.doc.highlight(color.map(CoreMarkColor::from));
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

    // ── the presentation vocabulary ───────────────────────────────────────────
    //
    // Six gestures and five queries over a document's *presentation*: alignment
    // and line spacing, which are the block's, and size, face and colour, which
    // are the run's. Each value is a **name** the renderer's theme resolves —
    // `center`, `1.5`, `large`, `serif`, `red` — or, for the four properties
    // whose type is open, the exact value the author asked for: `14pt`,
    // `Garamond`, `#c03030`, `1.3`. A name outlives the theme it was written
    // under and a value does not, which is the trade the *Other…* row makes
    // visible; see [`FontSize`] and its three peers.
    //
    // Each gesture edits one attribute key and keeps the rest, so a document
    // from elsewhere passes through the editor unharmed, and `nil` clears the
    // key. Each takes its enabled state from the matching [`Capabilities`] flag,
    // and its *lit* state from the query beside it — the queries read the
    // nearest node that names the property, so a control follows the caret into
    // a centred `<div>` the way the H1 light follows it into a heading.

    /// Align the caret's block, or return it to the theme's default with `nil`.
    ///
    /// A block property, so it is the caret's *block* whatever is selected: a
    /// line belongs to a block, and "centre this" with three words selected
    /// means the paragraph, not the words. Other `class` tokens on the block are
    /// kept. Gate on [`Capabilities::alignment`].
    pub fn set_alignment(&self, align: Option<Align>) -> DocView {
        let mut g = self.lock();
        g.doc.set_alignment(align.map(CoreAlign::from));
        g.view()
    }

    /// Set the line spacing of the caret's block, or return it to the theme's
    /// with `nil`. [`set_alignment`](Self::set_alignment)'s peer in every
    /// respect but the key. Gate on [`Capabilities::line_spacing`].
    ///
    /// `.step(.oneHalf)` from the menu's three, or `.ratio(1.3)` from an
    /// *Other…* field. A ratio that spells one of the three *is* that name, and
    /// a ratio of 1 is single spacing, which is absence and clears the key. A
    /// ratio the vocabulary cannot carry at all — `0`, `700`, a NaN — writes
    /// nothing, and the block's own spacing stands.
    pub fn set_line_spacing(&self, spacing: Option<LineHeight>) -> DocView {
        let mut g = self.lock();
        if let Some(spacing) = written(spacing, LineHeight::into_core) {
            g.doc.set_line_spacing(spacing);
        }
        g.view()
    }

    /// Set the size of the selected run, or of the caret's whole block when
    /// nothing is selected; `nil` returns it to the theme's own size.
    ///
    /// Size, face and colour are the *run's*, and the block's when no run is
    /// chosen — so "make this paragraph larger" is a press with the caret in it
    /// rather than a select-all first. With a selection the range is wrapped in
    /// an attributed span, or the span it already lies in is re-styled, never
    /// nested. Gate on [`Capabilities::font_size`].
    ///
    /// `.step(.large)` from the menu's seven, or `.points(14)` from an
    /// *Other…* field — a size in points, which is what the paper will show. A
    /// number of points the vocabulary cannot carry (0.01 to 655.35 is the
    /// range) writes nothing at all, and the run's own size stands: validate
    /// the field before calling, because from here the refusal is silent.
    pub fn set_font_size(&self, size: Option<FontSize>) -> DocView {
        let mut g = self.lock();
        if let Some(size) = written(size, FontSize::into_core) {
            g.doc.set_font_size(size);
        }
        g.view()
    }

    /// Set the face of the selected run, or of the caret's whole block.
    /// [`set_font_size`](Self::set_font_size)'s peer. Gate on
    /// [`Capabilities::font_family`].
    ///
    /// `.generic(.serif)` from the menu's four, or `.named("Garamond")` from
    /// the platform's font picker — which draws in that family where it is
    /// installed and in the theme's body face where it is not. A name that
    /// names nothing (`"   "`) writes nothing, and the run's own face stands.
    pub fn set_font_family(&self, font: Option<FontFace>) -> DocView {
        let mut g = self.lock();
        if let Some(font) = written(font, FontFace::into_core) {
            g.doc.set_font_family(font);
        }
        g.view()
    }

    /// Set the *text* colour of the selected run, or of the caret's whole block.
    /// [`set_font_size`](Self::set_font_size)'s peer, and **not**
    /// [`set_mark_color`](Self::set_mark_color): that one colours a highlight's
    /// background and needs a highlight to colour, this one paints the letters
    /// and needs nothing. They share the seven names on purpose. Gate on
    /// [`Capabilities::text_color`].
    ///
    /// `.named(.red)` from the seven swatches, or `.rgb(r:g:b:)` from the
    /// system colour picker — painted as written in both appearances, which is
    /// what "exact" costs.
    pub fn set_text_color(&self, color: Option<TextColor>) -> DocView {
        let mut g = self.lock();
        g.doc.set_text_color(color.map(TextColor::into_core));
        g.view()
    }

    /// Insert a page break at the caret — a leaf directive with no label, placed
    /// exactly as [`insert_thematic_break`](Self::insert_thematic_break) places a
    /// rule, a selection replaced by it and a bare paragraph parted at the caret
    /// first.
    ///
    /// A frontend that paginates opens a page at the row's directive mark and
    /// gives the row no height; one that does not draws the `⧉ page-break`
    /// placeholder every leaf directive gets. Gate on
    /// [`Capabilities::page_break`].
    pub fn insert_page_break(&self) -> DocView {
        let mut g = self.lock();
        g.doc.insert_page_break();
        g.view()
    }

    /// The alignment in force at the caret, or `nil` for the theme's default —
    /// which segment of an alignment control is lit.
    ///
    /// Read off the nearest node that names one: the caret's block, and the
    /// `div`s around it after that, so the control follows the caret into a
    /// centred `<div>`.
    pub fn alignment_at_caret(&self) -> Option<Align> {
        let mut g = self.lock();
        g.doc.alignment_at_caret().map(Align::from)
    }

    /// The line spacing in force at the caret, or `nil` for the theme's own —
    /// which entry a spacing menu shows ticked.
    /// [`alignment_at_caret`](Self::alignment_at_caret)'s peer.
    ///
    /// A `.ratio` is a spacing no row of the menu's three can tick, and is the
    /// author's own: the menu shows it as a row of its own above *Other…*.
    pub fn line_spacing_at_caret(&self) -> Option<LineHeight> {
        let mut g = self.lock();
        g.doc.line_spacing_at_caret().map(LineHeight::from)
    }

    /// The size in force at the caret, or `nil` for the theme's own — which
    /// entry a size menu shows ticked. Run-level, so the chain starts one node
    /// deeper: the attributed span the caret stands in, then its block, then the
    /// `div`s around it, the nearest winning.
    ///
    /// A `.points` is a size no row of the menu's seven can tick, and the menu
    /// shows it as a row of its own — "14 pt" — above *Other…*.
    pub fn font_size_at_caret(&self) -> Option<FontSize> {
        let mut g = self.lock();
        g.doc.font_size_at_caret().map(FontSize::from)
    }

    /// The face in force at the caret, or `nil` for the theme's body face.
    /// [`font_size_at_caret`](Self::font_size_at_caret)'s peer, and a `.named`
    /// is the family the author picked, shown as its own ticked row.
    pub fn font_family_at_caret(&self) -> Option<FontFace> {
        let mut g = self.lock();
        g.doc.font_family_at_caret().map(FontFace::from)
    }

    /// The *text* colour in force at the caret, or `nil` for the theme's — which
    /// swatch a text-colour control marks as the current one.
    /// [`font_size_at_caret`](Self::font_size_at_caret)'s peer, and not
    /// [`DocView::mark_color`], which reads a highlight's background off a
    /// `mark` node the caret is standing in. A `.rgb` is the author's own
    /// triple, which the palette shows as a swatch of its own.
    pub fn text_color_at_caret(&self) -> Option<TextColor> {
        let mut g = self.lock();
        g.doc.text_color_at_caret().map(TextColor::from)
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
    /// and link, and Markdown refuses the underline djot spells — so a toolbar
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

    /// Insert a fresh table at the caret — one header row, `rows` empty body
    /// rows, `cols` columns — and leave the caret in its first header cell.
    /// The one table verb that needs no table under the caret; gate it on
    /// [`Capabilities::table`] alone. See [`leaf_core::Doc::insert_table`] for
    /// the placement (a paragraph is parted around the caret, as for the rule)
    /// and for what a zero shape does.
    pub fn insert_table(&self, rows: u32, cols: u32) -> DocView {
        let mut g = self.lock();
        g.doc.insert_table(rows as usize, cols as usize);
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

    /// The source of the image the caret stands in — the `src` of an
    /// `![](cat.png)` or a `<img>`, exactly as the document spells it. `None`
    /// when the caret is in no image.
    ///
    /// Two hosts ask. An image prompt seeds from it, so editing an existing
    /// picture starts from its current URL rather than blank; and a host that
    /// gives an attachment a place of its own — a node, a page, a file
    /// inspector — asks it to answer "show me *this* one" from a menu raised
    /// over the body. See `LeafEditorModel.onShowMedia` in the Swift package.
    ///
    /// A caret resting just after a block image (its trailing stop) is already
    /// past it and gets `None`, which is the same half-open rule
    /// [`link_destination_at_caret`](Self::link_destination_at_caret) follows.
    pub fn image_destination_at_caret(&self) -> Option<String> {
        self.lock().doc.image_destination_at_caret()
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
        let mut o = g.snap_glyph_stop(off as usize);
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
        a = g.snap_glyph_stop(a);
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

    /// The UTF-16 index at which source offset `off` sits in the visible text —
    /// the string `text_in_range(0, doc_end_offset())` returns — which is the
    /// character space AppKit's `NSTextInputClient` and `NSAccessibility` speak.
    ///
    /// leaf's own handle is the source byte offset, and the two are not one
    /// scale apart: WYSIWYG hides delimiters, a block gap is spelled as one
    /// `\n`, and a character outside the BMP is two UTF-16 units. A frontend
    /// hands the system an `NSRange` converted with this and turns the ranges
    /// it gets back through `offset_for_utf16_index`, so Look Up, dictation, and
    /// VoiceOver all index the same text the frontend drew.
    pub fn utf16_index_for_offset(&self, off: u32) -> u32 {
        let mut g = self.lock();
        g.sync();
        let off = (off as usize).min(g.doc.source.len());
        match g.doc.view {
            View::Wysiwyg => g.doc.vmap.visible_utf16_len(0, off) as u32,
            View::Source => {
                let off = g.snap_stop(off);
                g.doc.source[..off].encode_utf16().count() as u32
            }
        }
    }

    /// The inverse of `utf16_index_for_offset`: the source offset of the
    /// visible character at UTF-16 `index`, or the document's end stop at or
    /// past the end of the text. An index inside a surrogate pair resolves to
    /// the character that owns it, and one on the `\n` a block gap is spelled
    /// with to the stop at the end of the block before it. Always a caret stop.
    pub fn offset_for_utf16_index(&self, index: u32) -> u32 {
        let mut g = self.lock();
        g.sync();
        let len = g.doc.source.len();
        let end = g.snap_stop(len);
        let index = index as usize;
        match g.doc.view {
            // A block separator resolves to the gap offset, which is no stop;
            // snapping lands it on the row end before it, the stop a caret
            // standing "after the last character" already means.
            View::Wysiwyg => g
                .doc
                .vmap
                .offset_at_visible_utf16(end, index)
                .map_or(end, |o| g.snap_stop(o)) as u32,
            View::Source => {
                let mut seen = 0usize;
                for (i, ch) in g.doc.source.char_indices() {
                    let n = ch.len_utf16();
                    if index < seen + n {
                        return i as u32;
                    }
                    seen += n;
                }
                end as u32
            }
        }
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
    /// delimiter (`**`, `` ` ``, `_`) contributes nothing, and a stop that
    /// draws no glyph — a row's end, a table cell's end — is spelled `'\n'`.
    /// Exactly one character per caret stop, so that for any two stops
    /// `text_in_range(a, b).chars().count() == distance_offset(a, b)`. That
    /// equality is what `UITextInput`'s word tokenizer relies on: it reads a
    /// window of this text, indexes into it by `offset(from:to:)`, and hands
    /// a character delta back through `position(from:offset:)` — see
    /// [`leaf_core::wysiwyg::VisualMap::visible_text`] for the rule and what
    /// a one-character drift did to a double-tapped word. The source view has
    /// nothing hidden to begin with, so there this is exactly the raw slice.
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
        // The colour rides `Run::mark_color`, not the class id: a renderer that
        // styles `mark` and nothing else still draws a coloured highlight.
        Role::Mark(_) => "mark".into(),
        Role::ListMarker => "list".into(),
        Role::QuoteGutter => "quote".into(),
        Role::Rule => "rule".into(),
        Role::Image => "image".into(),
        // A formula's stand-in: the one-character atom run an inline formula
        // renders to, or a display block's placeholder label. A renderer pairs
        // a `math` run with its [`MathView`] by `src` and draws the picture in
        // its place — see [`DocView::math`].
        Role::Math => "math".into(),
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
    // The map's own face table, for the family name a glyph carries only an id
    // for — threaded down beside `hls`, which travels the same road.
    let faces = vmap.faces();
    vmap.rows
        .iter()
        .map(|vrow| {
            Row {
                runs: runs_of(&vrow.glyphs, ss, se, hls, faces),
                decoration: vrow.decoration,
                code: vrow.code,
                code_lang: vrow.code_lang.clone(),
                directive: vrow.directive,
                directive_label: vrow.directive_label.clone(),
                // Straight off the row, not scanned out of its glyphs: an empty
                // heading has none to scan, and a renderer sizing the line by a
                // glyph's role drew `# ` at body height until it had text.
                heading: vrow.heading,
                // Off the row for the same reason, and more sharply: alignment
                // and spacing are properties of the *line*, so an empty
                // paragraph just centred has no run to carry them.
                align: vrow.align.map(|a| a.name().to_string()),
                line_height: vrow.line_height.map(|l| l.name().to_string()),
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
    faces: &CoreFaceTable,
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
                runs: runs_of(&seg, ss, se, hls, faces),
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
        runs: runs_of(&seg, ss, se, hls, faces),
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
    faces: &CoreFaceTable,
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
                        faces,
                    ));
                }
                cur = Some((key.0, key.1, key.2, g.src));
                buf.push(g.ch);
            }
        }
    }
    if let Some((style, was_sel, hl, src)) = cur {
        runs.push(make_run(
            buf,
            style,
            was_sel,
            hl.map(|i| &hls[i]),
            src,
            faces,
        ));
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

/// The formulas of a WYSIWYG frame — each atom and each display block, with
/// the `rows` span it occupies and the TeX to typeset. The peer of
/// [`wysiwyg_media`].
fn wysiwyg_math(vmap: &VisualMap) -> Vec<MathView> {
    vmap.math
        .iter()
        .map(|m| MathView {
            start_row: m.rows_span.start as u32,
            end_row: m.rows_span.end as u32,
            inline: m.glyph.is_some(),
            tex: m.tex.clone(),
            display: m.display,
            src: m.src as u32,
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
    let faces = vmap.faces();
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
                            lines: cell_lines(
                                &cell.glyphs,
                                cell.start,
                                cell.end,
                                ss,
                                se,
                                hls,
                                faces,
                            ),
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
    // Raw text carries no attributed span, so no run of it names a family and
    // the table it would be read out of is empty.
    let faces = CoreFaceTable::default();
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
                runs.push(make_run(
                    raw[..a].to_string(),
                    body,
                    false,
                    None,
                    start,
                    &faces,
                ));
            }
            runs.push(make_run(
                raw[a..b].to_string(),
                body,
                true,
                None,
                start + a,
                &faces,
            ));
            if b < raw.len() {
                runs.push(make_run(
                    raw[b..].to_string(),
                    body,
                    false,
                    None,
                    start + b,
                    &faces,
                ));
            }
        } else if !raw.is_empty() {
            runs.push(make_run(raw.to_string(), body, false, None, start, &faces));
        }

        rows.push(Row {
            runs,
            decoration: false,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            heading: None, // source view is raw text — no resolved heading rows
            align: None,   // …no attributes resolved onto a block…
            line_height: None,
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
    faces: &CoreFaceTable,
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
        mark_color: mark_color_name(style.role),
        token: style.token.map(|t| t.name().to_string()),
        size: style.size.map(|s| s.name().to_string()),
        font: style.font.and_then(|f| faces.spell(f).map(Cow::into_owned)),
        text_color: style.color.map(|c| c.name().to_string()),
    }
}

/// The name of a `mark` role's colour, for [`Run::mark_color`]. `None` for a
/// plain highlight and for every other role — the same answer, because neither
/// has a colour to name.
fn mark_color_name(role: Role) -> Option<String> {
    match role {
        Role::Mark(c) => c.map(|c| c.name().to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Arc<LeafDoc> {
        LeafDoc::new(src.to_string(), "markdown".to_string()).unwrap()
    }

    /// A token splits a run the way a style does — it *is* part of the style
    /// — and rides across as its class id. A block in a language no grammar
    /// covers is one plain `code` run with no token, as it always was.
    #[cfg(feature = "syntax")]
    #[test]
    fn a_highlighted_block_splits_its_runs_by_token() {
        let v = doc("```rust\nlet x = 1;\n```\n").set_unwrapped();
        let row = v.rows.iter().find(|r| r.code).expect("a code row");
        let classed: Vec<(&str, Option<&str>)> = row
            .runs
            .iter()
            .map(|r| (r.text.as_str(), r.token.as_deref()))
            .collect();
        assert_eq!(classed[0], ("let", Some("keyword")));
        assert!(row.runs.iter().all(|r| r.role == "code"), "{classed:?}");
        assert!(
            classed
                .iter()
                .any(|(t, k)| *t == "1" && *k == Some("constant")),
            "{classed:?}"
        );

        let v = doc("```text\nlet x = 1;\n```\n").set_unwrapped();
        let row = v.rows.iter().find(|r| r.code).unwrap();
        assert_eq!(row.runs.len(), 1);
        assert_eq!(row.runs[0].token, None);
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

    /// The block half of the presentation vocabulary, the whole way round:
    /// press, and the fact comes back on **every** row of the block as the name
    /// the document carries, with the query lighting the control that wrote it.
    ///
    /// The row rather than a run because an empty paragraph has no run — and a
    /// name rather than an index because the renderer's theme owns the ramp,
    /// which is the same division `mark_color` makes.
    #[test]
    fn the_block_vocabulary_crosses_on_the_row_and_comes_back_at_the_caret() {
        let d = doc("a centred paragraph\n");
        let v = d.set_alignment(Some(Align::Center));
        let aligned: Vec<&str> = v
            .rows
            .iter()
            .filter_map(|r| r.align.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(aligned, ["center"], "the token, on the paragraph's row");
        assert_eq!(d.alignment_at_caret(), Some(Align::Center));

        // A second property on the same block keeps the first: each gesture
        // edits one key and passes the rest back whole. (The caret is put back
        // in the paragraph first — in Markdown the attributes went onto a `div`
        // the press wrote *around* it, so the offset the caret kept is now that
        // opening line, which is no block of the document's.)
        let at = d.source().find("centred").unwrap() as u32;
        d.set_selection_offsets(at, at);
        let v = d.set_line_spacing(Some(LineHeight::Step(LineSpacing::OneHalf)));
        let row = v
            .rows
            .iter()
            .find(|r| r.align.is_some())
            .expect("the block");
        assert_eq!(row.align.as_deref(), Some("center"));
        assert_eq!(row.line_height.as_deref(), Some("1.5"));
        assert_eq!(
            d.line_spacing_at_caret(),
            Some(LineHeight::Step(LineSpacing::OneHalf))
        );

        // `nil` clears, and absence is the theme's default rather than a token
        // meaning "left".
        let at = d.source().find("centred").unwrap() as u32;
        d.set_selection_offsets(at, at);
        let v = d.set_alignment(None);
        assert!(v.rows.iter().all(|r| r.align.is_none()));
        assert_eq!(d.alignment_at_caret(), None);
        assert_eq!(
            d.line_spacing_at_caret(),
            Some(LineHeight::Step(LineSpacing::OneHalf)),
            "clearing one key leaves the other standing"
        );
    }

    /// The run half: size, face and colour ride the run beside `role`, so a
    /// renderer picks a font and a foreground without re-reading the document.
    /// With nothing selected the caret's whole block takes them, which is what
    /// makes "make this paragraph larger" one press.
    #[test]
    fn the_run_vocabulary_crosses_on_the_run_and_comes_back_at_the_caret() {
        let d = doc("big serif blue\n");
        // Each press with the caret back in the paragraph, as a frontend's is —
        // the first wrapped the block in a `div`, and the offset the caret kept
        // is that opening line rather than the text.
        let in_text = |d: &Arc<LeafDoc>| {
            let at = d.source().find("serif").unwrap() as u32;
            d.set_selection_offsets(at, at);
        };
        d.set_font_size(Some(FontSize::Step(SizeStep::Large)));
        in_text(&d);
        d.set_font_family(Some(FontFace::Generic(FontFamily::Serif)));
        in_text(&d);
        let v = d.set_text_color(Some(TextColor::Named(MarkColor::Blue)));

        let styled: Vec<(Option<&str>, Option<&str>, Option<&str>)> = v
            .rows
            .iter()
            .flat_map(|r| r.runs.iter())
            .filter(|r| !r.text.trim().is_empty())
            .map(|r| {
                (
                    r.size.as_deref(),
                    r.font.as_deref(),
                    r.text_color.as_deref(),
                )
            })
            .collect();
        assert_eq!(styled, [(Some("large"), Some("serif"), Some("blue"))]);

        assert_eq!(
            d.font_size_at_caret(),
            Some(FontSize::Step(SizeStep::Large))
        );
        assert_eq!(
            d.font_family_at_caret(),
            Some(FontFace::Generic(FontFamily::Serif))
        );
        assert_eq!(
            d.text_color_at_caret(),
            Some(TextColor::Named(MarkColor::Blue))
        );

        // A run's text colour is not a highlight's wash: nothing here is a
        // `mark`, so the palette that colours one reports nothing.
        assert!(!d.caret_in_mark());
        assert!(
            v.rows
                .iter()
                .flat_map(|r| r.runs.iter())
                .all(|r| r.mark_color.is_none())
        );
    }

    /// The other half of each open type: the value an *Other…* row writes, out
    /// through the gesture and back through both the query and the run view.
    /// The view's token is the canonical spelling, because a renderer's theme
    /// table is keyed by string and parses what it does not find.
    #[test]
    fn an_exact_value_crosses_as_its_own_token_and_comes_back_whole() {
        let d = doc("exact\n");
        let in_text = |d: &Arc<LeafDoc>| {
            let at = d.source().find("exact").unwrap() as u32;
            d.set_selection_offsets(at, at);
        };
        d.set_font_size(Some(FontSize::Points(14.0)));
        in_text(&d);
        d.set_font_family(Some(FontFace::Named("Garamond".to_string())));
        in_text(&d);
        d.set_text_color(Some(TextColor::Rgb {
            r: 0xc0,
            g: 0x30,
            b: 0x30,
        }));
        in_text(&d);
        let v = d.set_line_spacing(Some(LineHeight::Ratio(1.3)));

        let styled: Vec<(Option<&str>, Option<&str>, Option<&str>)> = v
            .rows
            .iter()
            .flat_map(|r| r.runs.iter())
            .filter(|r| !r.text.trim().is_empty())
            .map(|r| {
                (
                    r.size.as_deref(),
                    r.font.as_deref(),
                    r.text_color.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            styled,
            [(Some("14pt"), Some("Garamond"), Some("#c03030"))],
            "the value's own spelling, where a name stood before"
        );
        let spaced: Vec<&str> = v
            .rows
            .iter()
            .filter_map(|r| r.line_height.as_deref())
            .collect();
        assert_eq!(spaced, ["1.3"]);

        assert_eq!(d.font_size_at_caret(), Some(FontSize::Points(14.0)));
        assert_eq!(
            d.font_family_at_caret(),
            Some(FontFace::Named("Garamond".to_string()))
        );
        assert_eq!(
            d.text_color_at_caret(),
            Some(TextColor::Rgb {
                r: 0xc0,
                g: 0x30,
                b: 0x30
            })
        );
        assert_eq!(d.line_spacing_at_caret(), Some(LineHeight::Ratio(1.3)));
    }

    /// A named face reaching a run **inside a table cell** — the other road a
    /// run takes to a frontend, and the one an id resolved against the wrong
    /// table would quietly ruin: a cell's runs come through [`cell_lines`] and
    /// not through [`wysiwyg_rows`], so the map's [`CoreFaceTable`] has to be
    /// threaded down both. A grid whose faces all named nothing would draw in
    /// the body face and look like a theme that simply had no Garamond.
    #[test]
    fn a_named_face_reaches_a_run_inside_a_table_cell() {
        let d = doc("| a | b |\n|---|---|\n| one | two |\n");
        let at = d.source().find("one").unwrap() as u32;
        d.set_selection_offsets(at, at + 3);
        let v = d.set_font_family(Some(FontFace::Named("Garamond".to_string())));

        let faced: Vec<(&str, &str)> = v
            .tables
            .iter()
            .flat_map(|t| t.grid.iter())
            .flat_map(|r| r.cells.iter())
            .flat_map(|c| c.lines.iter())
            .flat_map(|l| l.runs.iter())
            .filter_map(|r| Some((r.text.trim(), r.font.as_deref()?)))
            .collect();
        assert_eq!(faced, [("one", "Garamond")]);
    }

    /// A value the vocabulary cannot carry writes **nothing at all**, and what
    /// the run already said stands — a typo in an *Other…* field is not a
    /// reason to throw away the size the author set a minute ago. The one
    /// value that clears is the one that *means* the theme's own: a ratio of
    /// 1, which is single spacing.
    #[test]
    fn a_value_outside_the_vocabulary_leaves_the_key_alone() {
        let d = doc("plain\n");
        let in_text = |d: &Arc<LeafDoc>| {
            let at = d.source().find("plain").unwrap() as u32;
            d.set_selection_offsets(at, at);
        };
        d.set_font_size(Some(FontSize::Points(14.0)));
        in_text(&d);
        assert_eq!(d.font_size_at_caret(), Some(FontSize::Points(14.0)));
        for refused in [700.0, 0.0, -3.0, f64::NAN] {
            d.set_font_size(Some(FontSize::Points(refused)));
            in_text(&d);
            assert_eq!(
                d.font_size_at_caret(),
                Some(FontSize::Points(14.0)),
                "{refused} is not a size, and the author's 14pt is not its casualty"
            );
        }

        // A ratio of 1 is single spacing, which is the theme's and has no
        // token: that one *is* absence, and clears. A ratio of 0 is not.
        d.set_line_spacing(Some(LineHeight::Ratio(1.5)));
        in_text(&d);
        assert_eq!(
            d.line_spacing_at_caret(),
            Some(LineHeight::Step(LineSpacing::OneHalf)),
            "a ratio that spells a name is that name"
        );
        d.set_line_spacing(Some(LineHeight::Ratio(0.0)));
        in_text(&d);
        assert_eq!(
            d.line_spacing_at_caret(),
            Some(LineHeight::Step(LineSpacing::OneHalf)),
            "nought is not a spacing, and refusing it keeps the block's own"
        );
        d.set_line_spacing(Some(LineHeight::Ratio(1.0)));
        in_text(&d);
        assert_eq!(d.line_spacing_at_caret(), None, "single is the theme's own");

        // And a name that spells a generic is that generic, whatever its case
        // — the reading a document's own `data-font` gets. A name that names
        // nothing is refused, and the face the run had stands.
        d.set_font_family(Some(FontFace::Named("  Serif ".to_string())));
        in_text(&d);
        assert_eq!(
            d.font_family_at_caret(),
            Some(FontFace::Generic(FontFamily::Serif))
        );
        d.set_font_family(Some(FontFace::Named("   ".to_string())));
        in_text(&d);
        assert_eq!(
            d.font_family_at_caret(),
            Some(FontFace::Generic(FontFamily::Serif))
        );

        // `nil` is the argument that clears, and it is the only one.
        d.set_font_size(None);
        d.set_font_family(None);
        in_text(&d);
        assert_eq!(d.font_size_at_caret(), None);
        assert_eq!(d.font_family_at_caret(), None);
    }

    /// A page break crosses as the leaf directive it is — the row a paginating
    /// frontend opens a page at.
    #[test]
    fn a_page_break_crosses_as_a_directive_of_its_own() {
        let d = doc("before\n\nafter\n");
        let v = d.insert_page_break();
        let names: Vec<&str> = v.directives.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["page-break"]);
        assert!(d.source().contains("page-break"));
    }

    /// One flag per new control, answered by the format — the toolbar builds
    /// itself from these rather than discovering each refusal on a press.
    /// Markdown spells all six; XML spells none of them, being parse-only.
    #[test]
    fn capabilities_answer_for_the_presentation_controls_too() {
        let md = doc("x\n").capabilities();
        assert!(md.alignment && md.line_spacing);
        assert!(md.font_size && md.font_family && md.text_color);
        assert!(md.page_break);

        let xml = LeafDoc::new("<a>x</a>".to_string(), "xml".to_string())
            .unwrap()
            .capabilities();
        assert!(!xml.alignment && !xml.line_spacing);
        assert!(!xml.font_size && !xml.font_family && !xml.text_color);
        assert!(!xml.page_break);
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
        let lines = cell_lines(&glyphs, 10, 16, 0, 0, &[], &CoreFaceTable::default());
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
        let lines = cell_lines(&trailing, 10, 15, 0, 0, &[], &CoreFaceTable::default());
        assert_eq!(lines.len(), 2);
        assert!(lines[1].runs.is_empty());
        assert_eq!((lines[1].start, lines[1].end), (15, 15));

        // No break: one line spanning the whole cell.
        let plain = [g('P', 10), g('e', 11)];
        let lines = cell_lines(&plain, 10, 12, 0, 0, &[], &CoreFaceTable::default());
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
    fn image_destination_at_caret_reads_the_image_under_the_caret() {
        let d = doc("![a](cat.png) after\n");
        d.set_selection_offsets(3, 3); // caret on the alt text
        assert_eq!(d.image_destination_at_caret().as_deref(), Some("cat.png"));
        d.set_selection_offsets(15, 15); // caret past the image, in the prose
        assert_eq!(d.image_destination_at_caret(), None);
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
    fn a_highlight_is_coloured_at_the_caret_and_the_frame_says_which() {
        // The whole crossing a colour palette makes: press the swatch, and the
        // frame that comes back names the colour so the swatch can light.
        let d = doc("a word b\n");
        d.set_selection_offsets(2, 6);
        d.toggle_mark();
        assert_eq!(d.source(), "a ==word== b\n");

        d.set_selection_offsets(5, 5); // inside the highlight
        assert!(d.caret_in_mark());
        let v = d.set_mark_color(Some(MarkColor::Red));
        assert_eq!(d.source(), "a ==🔴 word== b\n");
        assert_eq!(v.mark_color, Some(MarkColor::Red));

        // And the way back: no colour, still a highlight.
        let v = d.set_mark_color(None);
        assert_eq!(d.source(), "a ==word== b\n");
        assert_eq!(v.mark_color, None);
        assert!(d.caret_in_mark());
    }

    #[test]
    fn one_press_highlights_and_colours_and_one_undo_takes_it_back() {
        // What a toolbar swatch calls. The fold is core's, and it is what makes
        // the press reversible in one step rather than leaving an uncoloured
        // highlight behind.
        let d = doc("a word b\n");
        d.set_selection_offsets(2, 6);
        let v = d.highlight(Some(MarkColor::Purple));
        assert_eq!(d.source(), "a ==\u{1F7E3} word== b\n");
        assert_eq!(v.mark_color, Some(MarkColor::Purple));

        d.undo();
        assert_eq!(d.source(), "a word b\n");
    }

    #[test]
    fn the_palette_has_two_gates_and_they_ask_different_questions() {
        // `mark_color` is the format's answer and `caret_in_mark` the caret's.
        // A djot document spells the highlight and no colour for it, so the two
        // disagree there — which is the case a toolbar gating on either one
        // alone gets wrong.
        assert!(doc("x\n").capabilities().mark_color);
        let dj = LeafDoc::new("a {=word=} b\n".to_string(), "djot".to_string()).unwrap();
        assert!(dj.capabilities().mark, "djot writes the highlight");
        assert!(!dj.capabilities().mark_color, "and no colour on it");
        dj.set_selection_offsets(5, 5);
        assert!(dj.caret_in_mark(), "the caret is in one all the same");

        let d = doc("a word b\n");
        d.set_selection_offsets(3, 3);
        assert!(!d.caret_in_mark(), "no highlight to colour here");
        assert_eq!(d.set_mark_color(Some(MarkColor::Blue)).mark_color, None);
        assert_eq!(d.source(), "a word b\n", "and nothing written");
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
    fn utf16_indices_round_trip_through_the_visible_text_in_both_views() {
        // Hidden delimiters, an emoji outside the BMP, and a block gap: the
        // three ways an `NSRange` into the visible text and a source byte
        // offset part company.
        let d = doc("a **b\u{1F600}** c\n\nd\n");
        let end = d.doc_end_offset();
        let text = d.text_in_range(0, end);
        assert_eq!(text, "a b\u{1F600} c\nd");
        let total = text.encode_utf16().count() as u32;
        assert_eq!(d.utf16_index_for_offset(end), total);
        assert_eq!(d.offset_for_utf16_index(total), end);
        // Walk every stop: index it, and come back to the same stop.
        let mut off = 0u32;
        loop {
            let idx = d.utf16_index_for_offset(off);
            assert_eq!(
                d.offset_for_utf16_index(idx),
                off,
                "stop {off} via index {idx}"
            );
            let next = d.step_offset(off, 1);
            if next == off {
                break;
            }
            off = next;
        }
        // The 'c' comes after the hidden `**` and the two-unit emoji.
        let c = "a **b\u{1F600}** c".find(" c").unwrap() as u32 + 1;
        assert_eq!(
            d.utf16_index_for_offset(c),
            "a b\u{1F600} ".encode_utf16().count() as u32
        );

        // Source view: the text is the raw source, so the index is the plain
        // UTF-16 count of the bytes before the offset — delimiters included.
        d.toggle_view();
        assert_eq!(
            d.text_in_range(0, d.doc_end_offset()),
            "a **b\u{1F600}** c\n\nd\n"
        );
        assert_eq!(
            d.utf16_index_for_offset(c),
            "a **b\u{1F600}** ".encode_utf16().count() as u32
        );
        assert_eq!(d.offset_for_utf16_index(d.utf16_index_for_offset(c)), c);
        // Inside the emoji's surrogate pair resolves to the emoji itself.
        let emoji = "a **b".len() as u32;
        assert_eq!(
            d.offset_for_utf16_index(d.utf16_index_for_offset(emoji) + 1),
            emoji
        );
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

        // A window that is nothing but paragraph 1's end stop (no glyph in
        // it: it starts exactly at the end of paragraph 1's own last row)
        // is that stop's one character, the break itself.
        let gap_only = d.text_in_range(5, p2 as u32);
        assert_eq!(gap_only, "\n");

        // The break is the end stop's own character, not one inserted beside
        // it, so the equality the test above asserts holds across a
        // paragraph boundary too — the tokenizer's `position(from:offset:)`
        // walk lands exactly where the text it was handed put a boundary.
        for (a, b) in [(0u32, src.len() as u32), (3, p2 as u32 + 2), (5, p2 as u32)] {
            let text = d.text_in_range(a, b);
            let dist = d.distance_offset(a, b);
            assert_eq!(
                text.chars().count() as i32,
                dist,
                "text_in_range({a}, {b}) = {text:?} ({} chars) vs distance_offset {dist}",
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
    fn text_in_range_ends_a_line_at_each_table_cell_and_splits_none_inside() {
        // A cell's end reads as a line end — the tokenizer keeps `Status` and
        // `Tables` apart, and a tap past `Feature`'s last letter has no space
        // to step over into `Status` — and a table's rule rows, decoration
        // *inside* the one block, put nothing inside a cell (`Feature` once
        // came back as `F\neature`). The table's trailing stop — the caret
        // home past the last cell — draws no glyph either, so the document's
        // end is one more line end, the blank line under the table where the
        // caret past it stands.
        let d = doc("| Feature | Status |\n| --- | --- |\n| Tables | editable |\n");
        assert_eq!(
            d.text_in_range(0, d.doc_end_offset()),
            "Feature\nStatus\nTables\neditable\n"
        );
    }

    #[test]
    fn a_coloured_highlight_rides_out_as_a_name_beside_the_mark_role() {
        // The host draws the wash, so the colour has to reach it. It rides
        // `mark_color` rather than folding into `role` (`"mark-red"`) on
        // purpose: a renderer that only knows `"mark"` — every version of the
        // Swift one before this field existed — still draws the highlight.
        let d = doc("a ==\u{1F534} red== and ==plain== b\n");
        let runs = &d.view().rows[0].runs;
        let marks: Vec<(&str, Option<&str>)> = runs
            .iter()
            .filter(|r| r.role == "mark")
            .map(|r| (r.text.as_str(), r.mark_color.as_deref()))
            .collect();
        assert_eq!(marks, [("red", Some("red")), ("plain", None)]);
        assert!(
            runs.iter()
                .all(|r| r.role == "mark" || r.mark_color.is_none()),
            "nothing but a mark names a colour"
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

    /// The whole point of the record is that the numbers cross the boundary,
    /// so this checks the ones a host would show — and that a selection
    /// narrows them and no selection answers nothing at all.
    #[test]
    fn counts_cross_the_boundary_whole_and_selected() {
        let d = doc("a **bold** word\n\n- item\n");
        let c = d.counts();
        assert_eq!(
            (
                c.words,
                c.characters,
                c.characters_without_spaces,
                c.paragraphs
            ),
            (4, 15, 13, 2)
        );

        assert!(d.selection_counts().is_none(), "no selection, no counts");
        d.select_range(0, 10);
        let s = d.selection_counts().expect("a selection");
        assert_eq!((s.words, s.characters, s.paragraphs), (2, 6, 1));
    }

    #[test]
    fn an_inline_formula_is_a_math_run_paired_with_its_view_by_src() {
        // Off until the renderer says it paints in a line: the TeX as code.
        let d = doc("say $x+y$ here\n\nnext\n");
        d.set_selection_offsets(16, 16);
        let v = d.view();
        assert!(v.math.is_empty());
        assert!(
            v.rows[0]
                .runs
                .iter()
                .any(|r| r.role == "code" && r.text == "x+y")
        );
        // On: one `math` run, one character, and a view whose `src` is its.
        let v = d.set_inline_pictures(true);
        assert_eq!(v.math.len(), 1);
        let m = &v.math[0];
        assert!(m.inline);
        assert_eq!((m.start_row, m.end_row), (0, 1));
        assert_eq!(m.tex, "x+y");
        assert!(!m.display);
        assert_eq!(m.src, 4);
        let run = v.rows[0]
            .runs
            .iter()
            .find(|r| r.role == "math")
            .expect("a math run");
        assert_eq!(run.text, "∑");
        assert_eq!(run.src, m.src);
    }

    #[test]
    fn a_formula_on_the_caret_line_is_its_tex_and_no_view() {
        let d = doc("say $x+y$ here\n\nnext\n");
        d.set_inline_pictures(true);
        let v = d.set_selection_offsets(0, 0);
        assert!(v.math.is_empty());
        let text: String = v.rows[0].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "say $x+y$ here");
        assert!(
            v.rows[0]
                .runs
                .iter()
                .any(|r| r.role == "delimiter" && r.text == "$")
        );
    }

    #[test]
    fn a_display_block_is_rows_to_lay_over_and_measured_heights_grow_them() {
        let d = doc("$$\n\\int_0^1 x\n$$\n\nend\n");
        let v = d.set_selection_offsets(17, 17);
        assert_eq!(v.math.len(), 1);
        let m = &v.math[0];
        assert!(!m.inline);
        assert!(m.display);
        assert_eq!((m.start_row, m.end_row), (0, 1));
        assert_eq!(m.tex, "\n\\int_0^1 x\n");
        assert_eq!(m.src, 0);
        let after = d.set_math_rows(vec![MathHeight {
            tex: m.tex.clone(),
            rows: 4,
        }]);
        assert_eq!((after.math[0].start_row, after.math[0].end_row), (0, 4));
    }

    #[test]
    fn typeset_math_hands_back_a_picture_and_names_a_fault() {
        let p = typeset_math("E = mc^2".into(), false, 16.0, 0, 0, 0, 255).unwrap();
        assert!(p.svg.starts_with("<svg"));
        assert!(p.width > 3.0);
        assert!(p.height > 0.5);
        assert_eq!(p.depth, 0.0);
        match typeset_math("\\frac{".into(), true, 16.0, 0, 0, 0, 255) {
            Err(LeafError::Math { message, position }) => {
                assert!(!message.is_empty());
                assert!(position.is_some());
            }
            Err(other) => panic!("expected a math error, got {other}"),
            Ok(_) => panic!("expected a math error, got a picture"),
        }
    }
}
