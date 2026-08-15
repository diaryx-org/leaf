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

use leaf_core::style::{Role, Style as LStyle};
use leaf_core::wysiwyg::text_width;
use leaf_core::{
    Alignment, BlockKind, ColorScheme, Doc, Format, InlineKind, LineFlow as CoreLineFlow,
    MarkupMode as CoreMarkupMode, MediaKind as CoreMediaKind, View, VisualMap,
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
    /// Whether this run lies inside the active selection — so the renderer can
    /// paint a selection background without re-deriving it from offsets.
    pub sel: bool,
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
    fn to_core(self) -> Alignment {
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
    fn to_core(self) -> CoreMarkupMode {
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
    fn to_core(self) -> CoreLineFlow {
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
            View::Source => self.doc.source.split('\n').nth(row).unwrap_or("").to_string(),
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
        self.doc.source.split('\n').take(row).map(|l| l.len() + 1).sum()
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
                    Some(s[off..].grapheme_indices(true).nth(1).map_or(s.len(), |(i, _)| off + i))
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
            View::Wysiwyg => wysiwyg_rows(&self.doc.vmap, ss, se),
            View::Source => source_rows(&self.doc.source, ss, se),
        };
        // Structural tables, for a proportional renderer that draws its own grid;
        // none in the source view (the caret rides raw pipe text there).
        let tables = match self.doc.view {
            View::Wysiwyg => wysiwyg_tables(&self.doc.vmap, ss, se),
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
            other => return Err(LeafError::UnknownFormat { name: other.to_string() }),
        };
        let doc = Doc::from_source(source, format)
            .map_err(|e| LeafError::Parse { message: e.to_string() })?;
        Ok(Arc::new(LeafDoc {
            inner: Mutex::new(Inner { doc, width: Some(80), scheme: ColorScheme::Light }),
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
        g.scheme = if dark { ColorScheme::Dark } else { ColorScheme::Light };
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
            heights.into_iter().map(|h| (h.destination, h.rows.max(1) as usize)).collect(),
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

    // ── table editing ─────────────────────────────────────────────────────────

    /// Whether the caret is inside a table — for enabling the table controls.
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
        g.doc.table_set_alignment(alignment.to_core());
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
        g.doc.set_markup_mode(mode.to_core());
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
        g.doc.set_line_flow(mode.to_core());
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
        RowCol { row: row as u32, ch: ch as u32 }
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
        let (mut a, b, sign) = if from <= to { (from, to, 1i32) } else { (to, from, -1i32) };
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
        let target = if down { g.nav_below(row) } else { g.nav_above(row) };
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
fn wysiwyg_rows(vmap: &VisualMap, ss: usize, se: usize) -> Vec<Row> {
    vmap.rows
        .iter()
        .map(|vrow| {
            Row {
                runs: runs_of(&vrow.glyphs, ss, se),
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
                runs: runs_of(&seg, ss, se),
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
        runs: runs_of(&seg, ss, se),
        start: line_start.unwrap_or(cell_end) as u32,
        end: cell_end as u32,
    });
    lines
}

fn runs_of(glyphs: &[leaf_core::Glyph], ss: usize, se: usize) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<(LStyle, bool)> = None;
    for g in glyphs {
        let key = (g.style, g.src >= ss && g.src < se);
        match cur {
            Some(k) if k == key => buf.push(g.ch),
            _ => {
                if let Some((style, was_sel)) = cur.take() {
                    runs.push(make_run(std::mem::take(&mut buf), style, was_sel));
                }
                cur = Some(key);
                buf.push(g.ch);
            }
        }
    }
    if let Some((style, was_sel)) = cur {
        runs.push(make_run(buf, style, was_sel));
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
fn wysiwyg_tables(vmap: &VisualMap, ss: usize, se: usize) -> Vec<TableView> {
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
                            lines: cell_lines(&cell.glyphs, cell.start, cell.end, ss, se),
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

        let mut runs = Vec::new();
        if a < b {
            if a > 0 {
                runs.push(make_run(raw[..a].to_string(), body, false));
            }
            runs.push(make_run(raw[a..b].to_string(), body, true));
            if b < raw.len() {
                runs.push(make_run(raw[b..].to_string(), body, false));
            }
        } else if !raw.is_empty() {
            runs.push(make_run(raw.to_string(), body, false));
        }

        rows.push(Row {
            runs,
            decoration: false,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            heading: None, // source view is raw text — no resolved heading rows
            boundary: None, // …and no resolved block structure to divide
        });
        byte = end + 1; // skip the '\n' that `split` consumed
    }
    rows
}

/// Build a [`Run`] from an accumulated string and the core style it was drawn
/// with — the one place role and emphasis flags cross into the view shape.
fn make_run(text: String, style: LStyle, sel: bool) -> Run {
    Run {
        text,
        role: role_name(style.role),
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strike: style.strikethrough,
        sel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Arc<LeafDoc> {
        LeafDoc::new(src.to_string(), "markdown".to_string()).unwrap()
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
        assert!(head.runs.iter().all(|r| r.text.is_empty()), "the `# ` marker is hidden");
        assert_eq!(head.heading, Some(1));
        assert_eq!(v.rows[0].heading, None, "the paragraph above is not a heading");
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
        assert_eq!((v.caret_row, v.caret_ch), (4, 5), "the caret is on the heading's row");
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
        assert!(m.end_row > m.start_row, "the span must cover at least its label row");
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
        assert_eq!(off, "hi\n\n![](p.png)".len() as u32, "the stop past the picture");

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
        assert_eq!(undone.media.len(), 1, "and one undo brings the picture back");
    }

    #[test]
    fn measured_heights_grow_the_reserved_span() {
        // The height loop: core reserves one row until the renderer measures the
        // real view and reports back, because core does no I/O and cannot know.
        let d = doc("![a cat](cat.png)\n");
        let before = &d.view().media[0];
        assert_eq!(before.end_row - before.start_row, 1, "one row until measured");

        let after = d.set_media_rows(vec![MediaHeight {
            destination: "cat.png".to_string(),
            rows: 6,
        }]);
        let m = &after.media[0];
        assert_eq!(m.end_row - m.start_row, 6, "the span grew to what was measured");
    }

    #[test]
    fn inserted_media_comes_straight_back_out_as_media() {
        // Round trip across the boundary, the pair that matters: what Swift asks
        // to insert, Swift sees on the very next frame.
        let d = doc("\n");
        let v = d.insert_media(MediaKind::Audio, "take.mp3".to_string(), "a take".to_string());
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
        assert!(d.toggle_view().media.is_empty(), "no placeholders in the source view");
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
        let d = doc("# April 02, 2026\n\nAn interesting thing AI said to me:\n\n> a person… who journals\n");
        d.toggle_view(); // to the raw source view, where offsets index bytes directly
        let src = d.source();
        // The interior byte of the `…` — exactly the shape of the crash.
        let mid = src.find('…').expect("the ellipsis is in the fixture") + 1;
        assert!(!src.is_char_boundary(mid), "the fixture must be mid-character");

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
        let g = |ch, src| Glyph { ch, style: LStyle::default(), src, stop: true };
        // "a" at 10, a `<br>` at 11..15 (the break glyph), "b" at 15; cell 10..16.
        let glyphs = [g('a', 10), g('\n', 11), g('b', 15)];
        let lines = cell_lines(&glyphs, 10, 16, 0, 0);
        assert_eq!(lines.len(), 2, "one break makes two lines");
        assert_eq!((lines[0].start, lines[0].end), (10, 11), "line 1 ends at the break");
        assert_eq!((lines[1].start, lines[1].end), (15, 16), "line 2 begins past it");
        let text = |l: &TableCellLineView| l.runs.iter().map(|r| r.text.clone()).collect::<String>();
        assert_eq!(text(&lines[0]), "a");
        assert_eq!(text(&lines[1]), "b");

        // A trailing break leaves an empty last line homed at the cell's end.
        let trailing = [g('a', 10), g('\n', 11)];
        let lines = cell_lines(&trailing, 10, 15, 0, 0);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].runs.is_empty());
        assert_eq!((lines[1].start, lines[1].end), (15, 15));

        // No break: one line spanning the whole cell.
        let plain = [g('P', 10), g('e', 11)];
        let lines = cell_lines(&plain, 10, 12, 0, 0);
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
        assert_eq!(d.source().len(), before + 2, "a paragraph break is still \\n\\n");
    }

    #[test]
    fn link_destination_at_caret_reads_the_caret_link() {
        let d = doc("see [t](https://x.dev) ok\n");
        d.set_selection_offsets(5, 5); // caret on the link text "t"
        assert_eq!(d.link_destination_at_caret().as_deref(), Some("https://x.dev"));
        d.set_selection_offsets(0, 0); // caret on plain text
        assert_eq!(d.link_destination_at_caret(), None);
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
        assert_eq!(src.find("hello").unwrap(), 0, "paragraph 1 at the very start");
        let p2 = src[5..].find("hello").unwrap() + 5; // 7: paragraph 2's "hello"

        // A window straddling the tail of paragraph 1 ("lo") and the head of
        // paragraph 2 ("he").
        let text = d.text_in_range(3, p2 as u32 + 2);
        assert_ne!(text, "lohe", "the two paragraphs' words must not read as merged");
        assert!(
            text.chars().any(|c| !c.is_alphanumeric()),
            "a non-letter must separate the two paragraphs' words: got {text:?}"
        );
        assert_eq!(text, "lo\nhe", "exactly one separator opens the second paragraph's head");

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
        assert_eq!(d.distance_offset(5, p2 as u32), 1, "one Right crosses the whole gap");
    }
}
