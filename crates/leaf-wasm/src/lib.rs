//! leaf-wasm — the WebAssembly frontend binding for leaf.
//!
//! This is the browser analogue of `leaf-tui`'s `style.rs` + `ui.rs`: it takes
//! `leaf-core`'s frontend-neutral [`Doc`] — the byte-offset caret model and the
//! AST→glyph [`VisualMap`] — and exposes it across the wasm boundary in the
//! shape a web renderer wants. Core stays the single source of truth for the
//! text, the caret math, and the offset⇄position mapping; the JS side only
//! paints glyphs and forwards key/mouse events back in, exactly as the TUI and
//! gpui frontends do.
//!
//! ## The boundary is style *runs*, not glyphs
//!
//! [`Doc::build_visual`] resolves the document to rows of per-character glyphs,
//! each tagged with a semantic [`Role`] and the author's emphasis. Sending one
//! JS object per character would make every keystroke O(document) in boundary
//! crossings. Instead [`LeafDoc::view`] coalesces each row's glyphs into maximal
//! **runs** of identical style (the same merge the TUI does when it builds
//! ratatui `Span`s) and ships those — a handful of objects per line. The JS
//! renderer maps each run's `role` to a CSS class and its emphasis flags to
//! font styling, the web counterpart of `to_ratatui` / `text_run`.
//!
//! ## Core owns the grid; the browser owns the pixels
//!
//! Core lays a row out in whole character *columns* (a terminal cell measure),
//! and every offset⇄position method — [`Doc::caret_pos`], [`Doc::click`],
//! vertical motion — speaks that grid. It wraps each logical line to a column
//! budget and hands back rows, a caret at `(row, col)`, and the up/down goal
//! math, and it stays the sole authority on all of that. What it deliberately
//! does *not* dictate is presentation: a column is a semantic position, not a
//! pixel offset.
//!
//! So the renderer is *proportional*, the web peer of `leaf-gpui`'s `style.rs`:
//! body text in a real proportional family, headings distinguished by **size**
//! (a per-level scale ramp) and weight rather than a recoloured cell, code in a
//! monospace family with a tinted panel. Because the glyphs no longer sit on a
//! fixed pixel grid, the JS side never multiplies `col × cell_width`; it lets the
//! browser shape each row and reads the caret's pixel position back out of the
//! DOM (a collapsed `Range` at the caret column), and hit-tests a click through
//! `caretRangeFromPoint`, translating the DOM node+offset back to core's
//! `(row, col)` by counting glyph columns. Core measures nothing in pixels; the
//! browser positions nothing in the model — the same division of labour gpui
//! keeps between the document and its own visual layout. Each row carries its
//! [`Row::heading`] level so the whole line can be sized as one unit, mirroring
//! how gpui shapes a heading's line at a single larger size.

use std::borrow::Cow;

use leaf_core::style::{
    Align as CoreAlign, Baseline, FaceTable as CoreFaceTable, FontFace as CoreFontFace,
    FontSize as CoreFontSize, LineHeight as CoreLineHeight, MarkColor as CoreMarkColor, Role,
    Style as LStyle, TextColor as CoreTextColor,
};
use leaf_core::wysiwyg::text_width;
use leaf_core::{
    Alignment, BlockClass, BlockKind, ColorScheme, Doc, Format, Glyph, Highlight as CoreHighlight,
    InlineKind, LineFlow as CoreLineFlow, MarkupMode as CoreMarkupMode, MediaKind, SourceMap,
    TextCounts as CoreTextCounts, View, VisualMap,
};
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;
use unicode_segmentation::UnicodeSegmentation;
use wasm_bindgen::prelude::*;

/// One maximal span of same-styled glyphs on a visual row — the unit the JS
/// renderer turns into a single styled DOM node.
#[derive(Clone, Debug, PartialEq, Serialize, Tsify)]
pub struct Run {
    /// The run's text, glyphs concatenated in column order.
    text: String,
    /// The glyph's semantic role as a renderer class id: `body`, `h1`…`h6`,
    /// `code`, `link`, `mark`, `list`, `quote`, `rule`.
    role: String,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    /// Raised off the baseline and drawn smaller — a footnote reference's `[1]`,
    /// or an author's `^x^`. Mutually exclusive with [`Self::sub`]; core's
    /// `Baseline` is one value, and these are its two non-default cases
    /// flattened to the flag shape the rest of this record is spelled in.
    sup: bool,
    /// Lowered off the baseline and drawn smaller — an author's `~x~`.
    sub: bool,
    /// The byte offset in the source this run's first glyph came from.
    ///
    /// What a run *means*, as opposed to how it looks: a `link` role says a span
    /// is drawn as a link but not where it points, and the only way back to that
    /// is the source. A renderer making a link followable, or a footnote's `[1]`
    /// clickable, pairs this with [`LeafDoc::link_destination_at`] or
    /// [`LeafDoc::footnote_at`].
    ///
    /// The alternative was for the renderer to count along the row's text and
    /// ask [`LeafDoc::offset_for_pos`], which means converting between three
    /// units that agree only on ASCII: this is a byte offset, the run's text is
    /// UTF-16 code units, and a row's column is a *display* cell (a wide CJK
    /// glyph is two). Handing the offset over is exact and O(1).
    ///
    /// `0` for the runs of the source view, whose rows are split from raw text
    /// rather than laid out from glyphs.
    src: usize,
    /// Whether this run lies inside the active selection — so the renderer can
    /// paint a selection background without the JS side re-deriving it from
    /// offsets. Selection splits a run the same way a style change does.
    sel: bool,
    /// The id of the host highlight covering this run, if one does — see
    /// [`LeafDoc::set_highlights`]. A highlight splits a run the way the
    /// selection does, so a wash begins and ends exactly on its bytes.
    hl: Option<String>,
    /// That highlight's rendering hint (`#RRGGBB`, or `None` for the theme's
    /// default wash), carried beside the id so a renderer needs no lookup.
    hl_color: Option<String>,
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
    mark_color: Option<String>,
    /// What a `code` run is to the language its fenced block is written in —
    /// `"punctuation"`, `"keyword"`, `"entity"`, `"support"`, `"constant"`,
    /// `"string"`, `"comment"`, `"invalid"` — or absent for a run the grammar
    /// left plain, for every run of a block in a language no grammar covers,
    /// for inline code, and for every other role.
    ///
    /// A class id like `role`, and beside it for the reason `mark_color` is: a
    /// renderer that knows nothing about tokens still draws the run as the code
    /// it is, and one that does keys a palette on the name.
    token: Option<String>,
    /// How large this run is set — one of CSS's seven `<absolute-size>`
    /// keywords (`"xx-small"`, `"x-small"`, `"small"`, `"large"`, `"x-large"`,
    /// `"xx-large"`, `"xxx-large"`), or the exact size the author asked for
    /// (`"14pt"`, `"13.5pt"`) — and absent for the theme's own size, which is
    /// every run there was before the presentation vocabulary.
    ///
    /// A keyword is a rule the stylesheet can enumerate —
    /// `[data-size="large"] { font-size: large }` — and says how much bigger
    /// while the stylesheet says how big. A `pt` size cannot be enumerated, and
    /// is a transcription of `font-size: 14pt` the renderer sets inline beside
    /// the attribute: that is the portability the author traded away knowingly,
    /// and `presentation.css` alone draws the keywords and not the values.
    size: Option<String>,
    /// The face this run is set in — one of CSS's four generics (`"serif"`,
    /// `"sans-serif"`, `"monospace"`, `"cursive"`), or a family the author
    /// named (`"Garamond"`) — and absent for the theme's body face.
    ///
    /// A generic is a rule (`font-family: serif`) and the browser's fallback
    /// chain does the resolving a document should never do for itself. A family
    /// name is `font-family: Garamond` inline, for [`Self::size`]'s reason, and
    /// falls back to the page's own face where it is not installed.
    font: Option<String>,
    /// The run's *foreground* colour — one of the seven names
    /// [`Self::mark_color`] carries, or six lowercase hex digits behind a `#`
    /// (`"#c03030"`) — and absent for the theme's text colour.
    ///
    /// Not `mark_color`, though they share a vocabulary on purpose: that is a
    /// highlight's *background* and reaches a run through its `mark` role, this
    /// is what the letters themselves are painted. A renderer with a red for a
    /// highlight has a red for text, and both should be that red — which on the
    /// web is one custom property read by both rules. A triple is `color:
    /// #c03030` inline and is painted as written in both appearances, which is
    /// what "exact" means.
    text_color: Option<String>,
}

/// A selection cited out of the source — the text, a little of what
/// surrounded it, and the byte range it came from. The wasm shape of
/// `leaf_core::Quote`; see [`LeafDoc::selection_quote`].
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct SelectionQuote {
    /// The selected source, verbatim.
    exact: String,
    /// What immediately preceded it — empty at the document's start.
    prefix: String,
    /// What immediately followed it — empty at the document's end.
    suffix: String,
    /// Byte offset in the source where the selection begins.
    start: usize,
    /// Byte offset where it ends (exclusive).
    end: usize,
}

/// How much writing there is — over the whole document, or over the
/// selection. The wasm shape of `leaf_core::TextCounts`; see
/// [`LeafDoc::counts`] for what is counted and what isn't.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct TextCounts {
    /// Words, by UAX#29 word segmentation: a segment holding at least one
    /// letter or digit, so `don't` is one and a lone dash is none. A
    /// hyphenated compound is two, which is what the algorithm says.
    words: usize,
    /// Characters as a reader counts them — grapheme clusters, spaces
    /// included. An emoji family and an accented letter are each one.
    characters: usize,
    /// The same, less every whitespace grapheme.
    characters_without_spaces: usize,
    /// Block-level containers holding at least one non-whitespace character:
    /// a paragraph, a heading, each list item, each paragraph inside a
    /// blockquote, a whole code block, a whole table.
    paragraphs: usize,
}

impl From<CoreTextCounts> for TextCounts {
    fn from(c: CoreTextCounts) -> Self {
        TextCounts {
            words: c.words,
            characters: c.characters,
            characters_without_spaces: c.characters_without_spaces,
            paragraphs: c.paragraphs,
        }
    }
}

/// A host-painted range of the source, as [`LeafDoc::set_highlights`] takes
/// it — the wasm shape of `leaf_core::Highlight`.
#[derive(Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
pub struct HighlightIn {
    /// Byte offset in the source where the wash begins.
    pub start: usize,
    /// Byte offset where it ends (exclusive).
    pub end: usize,
    /// The host's name for it, handed back on activation. Opaque to leaf.
    pub id: String,
    /// A rendering hint (`#RRGGBB`), or absent for the theme's default wash.
    pub color: Option<String>,
    /// A margin glyph's name (a CSS class, for this binding's frontends), or
    /// absent for wash-only ink. The marker — not the wash — is what
    /// activates a highlight; see `leaf_core::Highlight::marker`.
    pub marker: Option<String>,
}

/// A host-painted range as [`LeafDoc::highlights`] reports it back — the
/// outbound twin of [`HighlightIn`], serialized rather than deserialized.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct HighlightOut {
    pub start: usize,
    pub end: usize,
    pub id: String,
    pub color: Option<String>,
    pub marker: Option<String>,
}

/// One visual line of a table cell — a cell holds more than one only when an
/// in-cell `<br>` splits it.
#[derive(Serialize, Tsify)]
pub struct TableCellLineView {
    runs: Vec<Run>,
    /// The source offsets bounding this line's content — the caret home at its
    /// start and the stop just past its end.
    start: usize,
    end: usize,
}

/// One cell of a table's structural grid: its content as visual lines, the
/// column alignment its text honours, and the source range the whole cell
/// occupies (where a click or the caret lands).
#[derive(Serialize, Tsify)]
pub struct TableCellView {
    lines: Vec<TableCellLineView>,
    /// `"left"`, `"right"`, `"center"`, or `"default"`.
    align: String,
    start: usize,
    end: usize,
}

/// One row of a table's structural grid; a header row draws bold and is ruled
/// off from the body below it.
#[derive(Serialize, Tsify)]
pub struct TableRowView {
    head: bool,
    cells: Vec<TableCellView>,
}

/// A table described *structurally* rather than as the monospace box-glyph
/// picture that spells it in [`DocView::rows`].
///
/// The picture is exactly right on a fixed-cell surface and unfixable off one:
/// in a proportional font the `│` of one row and the `│` of the next land at
/// different x, and the grid shears. So the browser — which is proportional —
/// **skips the rows in `[start_row, end_row)`** and lays out a real `<table>`
/// from this instead, exactly as it lays a real `<img>` over a [`MediaView`]'s
/// placeholder row. The two describe the same cells at the same source offsets,
/// so the caret lands identically either way. See [`leaf_core::TableInfo`].
#[derive(Serialize, Tsify)]
pub struct TableView {
    /// The [`DocView::rows`] indices the box-drawn picture occupies — the rows a
    /// grid-drawing renderer skips.
    start_row: usize,
    end_row: usize,
    grid: Vec<TableRowView>,
}

/// One `{key=value}` attribute of a [`DirectiveView`]. A bare attribute
/// (`{public}`) has an empty value, which a consumer reads as a flag.
#[derive(Serialize, Tsify)]
pub struct DirectiveAttr {
    key: String,
    value: String,
}

/// A leaf directive (`::name{…}`) — a standalone block with no body, drawn in
/// [`DocView::rows`] as a one-row `⧉ name` placeholder. A renderer that knows
/// the host app's vocabulary reads this and paints the real thing over the rows
/// in `[start_row, end_row)` — an `<iframe>` for diaryx's `::embed{src=…}`, say
/// — exactly as a grid-drawing one replaces a [`TableView`]'s picture rows. One
/// that doesn't just paints the placeholder.
///
/// Core resolves nothing here and neither does this layer: the vocabulary
/// belongs to the app. See [`leaf_core::DirectiveInfo`].
#[derive(Serialize, Tsify)]
pub struct DirectiveView {
    start_row: usize,
    end_row: usize,
    /// The directive's type (`embed`, `toc`, `vis`), no leading colons.
    name: String,
    /// Its `[label]` text, or empty — what the placeholder row shows.
    label: String,
    attrs: Vec<DirectiveAttr>,
}

/// What a drawn block boundary separates: the kinds of the blocks either side,
/// as renderer class ids (`paragraph`, `heading`, `list`, `list-item`, `quote`,
/// `code`, `table`, `media`, `directive`, `rule`, `footnote`, `other`). The pair
/// a frontend multiplies by its own spacing. See [`leaf_core::Boundary`].
#[derive(Clone, Debug, PartialEq, Serialize, Tsify)]
pub struct BoundaryView {
    above: String,
    below: String,
}

/// One visual line: its styled runs plus the row-level flags a frontend draws
/// chrome from.
#[derive(Clone, Debug, PartialEq, Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct Row {
    runs: Vec<Run>,
    /// Drawn but holds no caret (a table rule, a block-gap blank line): the
    /// renderer skips it for click/caret math. See [`leaf_core::VRow`].
    decoration: bool,
    /// A fenced/indented code-block line — the renderer draws a tinted, bordered
    /// panel around each maximal run of these.
    code: bool,
    /// A fenced block's language, carried on the block's first code row only.
    code_lang: Option<String>,
    /// This row belongs to a `:::name{.class}` directive container — twig's
    /// generic fenced-div block, whose meaning belongs to the host app. The
    /// renderer draws a tinted panel around each maximal run of these, as it
    /// does for a code block.
    directive: bool,
    /// A directive container's space-joined attrs, on the block's first row
    /// only — the `code_lang` pattern. `null` on every other row and on a
    /// container with no such attrs.
    directive_label: Option<String>,
    /// What this row divides, on the blank rows a block boundary is *drawn*
    /// with, and `null` on every other row.
    ///
    /// A boundary's *height* is a frontend decision but its *kind* is not.
    /// Typography spaces a gap by what it separates — the margin above a heading
    /// is wider than the one between two paragraphs, so the heading groups with
    /// the text it introduces. Core knows, having just walked the AST to emit
    /// the row; a renderer that instead sniffs the row's glyphs for emptiness is
    /// re-deriving structure core already published, and three frontends
    /// sniffing separately is three chances to disagree about one document.
    boundary: Option<BoundaryView>,
    /// The heading level (1–6) if this row belongs to a heading block, else
    /// `None`. A proportional renderer sizes the *whole* row from this — line
    /// height and all — the web analogue of gpui shaping a heading's line at one
    /// larger size, so an inline `` `code` `` run inside a heading still reads at
    /// the heading's size rather than dropping to body. (The per-run `role`
    /// already carries `h1`…`h6` too, but that can't tell the renderer how tall
    /// to make a row whose runs are mixed.)
    heading: Option<u8>,
    /// How this row's block is aligned across the measure — `"center"`,
    /// `"right"`, `"justify"` — and `null` for the theme's default, which is
    /// left. On every row the block emits.
    ///
    /// The token is the CSS class, so the renderer puts it on the row element
    /// and `.center { text-align: center }` does the rest. A *row* fact and not
    /// a run one for `heading`'s reason, and more sharply: alignment is a
    /// property of the line, not of the letters on it, so an empty paragraph the
    /// author has just centred carries it with no run to hang it on.
    align: Option<String>,
    /// How far apart this row's block sets its lines, as a multiple of the
    /// theme's own line height — the menu's three (`"1.15"`, `"1.5"`, `"2"`)
    /// or any other positive decimal the author asked for (`"1.3"`) — and
    /// `null` for the theme's spacing, which is what `"1"` would mean and is
    /// why it is never written. On every row the block emits.
    ///
    /// The token *is* the ratio, so the stylesheet's rule for one of the three
    /// is `[data-line-height="1.5"] { line-height: 1.5 }` and a value it cannot
    /// enumerate is `line-height: 1.3` inline, for [`Run::size`]'s reason.
    line_height: Option<String>,
}

/// One `<source>` alternative of a block media element, as JS sees it — a
/// candidate URL plus whichever of the two things HTML picks a `<source>` by.
/// The renderer emits these as real `<source>` children and lets the browser
/// choose, which is the one place the web frontend has it easier than the
/// native ones: matching a media query or a codec is what a browser is for.
#[derive(Serialize, Tsify)]
pub struct MediaSourceView {
    /// The `media="…"` query, or empty for an unconditional source.
    media: String,
    /// The candidate URL (a `<picture>` `srcset` or a `<video>`/`<audio>` `src`).
    src: String,
    /// The `type="…"` MIME, or empty when the source declares none.
    mime: String,
}

/// One block-level image, video, or audio: which rows core reserved for it and
/// what to build there. The web peer of [`leaf_core::MediaInfo`] — the renderer
/// **skips the rows in `[row, row + rows)`** and positions one real `<img>`,
/// `<video>`, or `<audio>` over them, instead of painting the `🖼`/`🎬`/`🔊`
/// placeholder glyphs core put there for a surface that can't.
#[derive(Serialize, Tsify)]
pub struct MediaView {
    /// The first [`DocView::rows`] row of the placeholder — where the element is
    /// positioned.
    row: usize,
    /// How many rows the placeholder spans, the label row included. Core's
    /// default is 1 until the renderer measures the real element and reports a
    /// height back through [`LeafDoc::set_media_rows`].
    rows: usize,
    /// `"image"`, `"video"`, or `"audio"` — which element to build.
    kind: String,
    /// The URL to load, already resolved against the document's colour scheme
    /// (see [`LeafDoc::set_color_scheme`]). Empty only when a `<video>`/`<audio>`
    /// named no `src` and no `<source>` either, which is a broken document.
    src: String,
    /// A `<video>`'s poster frame URL, or empty — passed through to the
    /// element's `poster` attribute so the browser shows a still before play.
    poster: String,
    /// The alt text / fallback text, for the `<img alt>` or the element's body.
    alt: String,
    /// The `<source>` alternatives, in document order; empty for a plain image.
    sources: Vec<MediaSourceView>,
}

/// A visual position: a row of [`DocView::rows`] and a UTF-16 offset into its
/// text, the pair a DOM `Range` is built from.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct RowCol {
    row: usize,
    ch: usize,
}

/// The rows a source range covers, both ends **inclusive** — what a renderer
/// slices out of a frame to draw a block somewhere other than where it sits: a
/// footnote peek, a link preview.
///
/// Inclusive rather than half-open because the answer is "these rows", not "up
/// to here": every caller wants `rows.slice(first, last + 1)`, and a `last` one
/// past the end would be a second thing to get wrong at each of them.
/// `last >= first` always, so the pair is never empty.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct RowRange {
    first: usize,
    last: usize,
}

/// Where a locator lands — what [`LeafDoc::locate`] answers with, and the mirror
/// of [`leaf_core::Landing`].
///
/// A span rather than an offset because the two things a host does with a
/// locator want different halves of it: following one puts a caret at `start`,
/// while previewing one draws the rows between `start` and `end`. Only the first
/// can be recovered from an offset alone.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct LandingView {
    start: usize,
    end: usize,
}

/// The heading a place sits under — what [`LeafDoc::heading_at`] answers
/// with, and the mirror of [`leaf_core::Heading`]. `text` is the heading's
/// words with their markup stripped, what a `#slug` is made from; `start` and
/// `end` are the heading block's own span.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct HeadingView {
    text: String,
    level: u32,
    start: usize,
    end: usize,
}

impl From<leaf_core::Heading> for HeadingView {
    fn from(h: leaf_core::Heading) -> Self {
        HeadingView {
            text: h.text,
            level: h.level,
            start: h.span.start,
            end: h.span.end,
        }
    }
}

/// Where a dragged block would land — what `dropTargetAt` answers with, the
/// web peer of [`leaf_core::DropTarget`]. `offset` is `moveBlock`'s `to`;
/// `row` is the rendered row to draw the indicator above — `rows.length`
/// for a drop below everything.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct DropTargetView {
    offset: usize,
    row: usize,
}

impl From<leaf_core::DropTarget> for DropTargetView {
    fn from(t: leaf_core::DropTarget) -> Self {
        DropTargetView {
            offset: t.offset,
            row: t.row,
        }
    }
}

impl From<leaf_core::Landing> for LandingView {
    fn from(l: leaf_core::Landing) -> Self {
        LandingView {
            start: l.start,
            end: l.end,
        }
    }
}

/// A footnote reference and the note it names — what [`LeafDoc::footnote_at`]
/// answers with, and the mirror of [`leaf_core::FootnoteRef`].
///
/// A reference whose definition the document is missing still comes back, with
/// its `label` and no `text`: that a `[^99]` names nothing is a thing to tell
/// the reader, and it is not the same as the caret standing on no reference at
/// all (which is `undefined`).
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct FootnoteView {
    /// The reference's label — the `1` of `[^1]`, without the `^` or brackets.
    label: String,
    /// The note's body as source text, or `null` when nothing defines it.
    text: Option<String>,
    /// The byte offset the note's body starts at, for a "go to note" that moves
    /// the caret there. `null` alongside a `null` `text`.
    offset: Option<usize>,
    /// Where the body ends, exclusive. With `offset` this bounds the note, so a
    /// renderer can map the pair through [`LeafDoc::row_range_for`] to the
    /// *rendered rows* it occupies and draw those — the note with its markup
    /// resolved, rather than the asterisks and backticks `text` carries.
    end: Option<usize>,
}

impl From<leaf_core::FootnoteRef> for FootnoteView {
    fn from(f: leaf_core::FootnoteRef) -> Self {
        FootnoteView {
            label: f.label,
            text: f.text,
            offset: f.offset,
            end: f.end,
        }
    }
}

/// A footnote definition and the reference that sends a reader to it — the other
/// half of [`FootnoteView`]'s round trip: that one carries a reader down to the
/// note, this one carries them back up. A definition nothing cites still comes
/// back, with its `label` and no `offset`.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct FootnoteDefView {
    label: String,
    /// The byte offset the first reference starts at. `null` for a note nothing
    /// refers to.
    offset: Option<usize>,
}

impl From<leaf_core::FootnoteDef> for FootnoteDefView {
    fn from(f: leaf_core::FootnoteDef) -> Self {
        FootnoteDefView {
            label: f.label,
            offset: f.offset,
        }
    }
}

/// Which formatting controls this document's format can spell — the toolbar's
/// enabled state, one flag per button, from [`LeafDoc::capabilities`]. Mirrors
/// [`leaf_core::Capabilities`], where the reasoning lives.
///
/// `into_wasm_abi` so the generated `.d.ts` types the getter as this record
/// rather than `any`: the renderer destructures it once per document and keys
/// each button's `disabled` off a field.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct CapabilitiesView {
    bold: bool,
    italic: bool,
    code: bool,
    mark: bool,
    underline: bool,
    strike: bool,
    superscript: bool,
    subscript: bool,
    /// Both the heading levels and "make this a paragraph" — one gesture in
    /// core, so one flag here.
    heading: bool,
    blockquote: bool,
    bullet_list: bool,
    ordered_list: bool,
    /// Giving an item a checkbox and ticking one, including a *click* on a
    /// rendered box.
    task: bool,
    link: bool,
    /// The highlight *palette* — `setMarkColor`. Narrower than `mark`: Markdown
    /// spells a colour on a highlight (`==🔴 text==`) and djot spells only the
    /// highlight. Gate the swatches on this *and* `caretInMark`.
    mark_color: bool,
    /// Covers `insertMedia` too.
    image: bool,
    thematic_break: bool,
    /// The footnote button — writes the `[^1]` and the definition it needs.
    footnote: bool,
    /// The code-block button — `toggleCodeBlock`. Pair with the frame's
    /// `code_block` for its lit state.
    code_block: bool,
    code_language: bool,
    /// The grid controls. Gate them on this *and* `caretInTable`: this asks
    /// whether the format's tables are editable, that whether the caret is in
    /// one — an HTML `<table>` answers yes to the second and no to the first.
    table: bool,
    /// Shift+Return inside a cell.
    cell_line_break: bool,
    /// The alignment control — `set_alignment`. Every format leaf opens but XML
    /// spells a block's attributes.
    alignment: bool,
    /// The line-spacing menu — `set_line_spacing`. The same gesture as
    /// `alignment` and so the same answer, and its own flag because a toolbar
    /// dims controls one at a time.
    line_spacing: bool,
    /// The size menu — `set_font_size`. **Narrower than the block pair**: it
    /// wraps a selection in an attributed span, which AsciiDoc has no slot for,
    /// so this is `false` there while `alignment` is `true`. The block-level form
    /// of the same property — the caret in a paragraph, nothing selected — still
    /// works, which is why the flag describes the control rather than the caret.
    font_size: bool,
    /// The face menu — `set_font_family`. A span, as `font_size` is.
    font_family: bool,
    /// The text-colour swatches — `set_text_color`. A span again, and not to be
    /// confused with `mark_color`: that is a highlight's background and rides the
    /// `mark` node, this is a run's foreground and rides an attributed span.
    text_color: bool,
    /// The page-break button — `insert_page_break`. Markdown, djot, HTML and
    /// AsciiDoc, each spelling it its own way and each drawn as the same
    /// placeholder row.
    page_break: bool,
    /// Moving a block — `moveBlock`, `moveBlockUp`, `moveBlockDown`, and a
    /// drag. Every format with blocks a caret can name; XML has none.
    move_block: bool,
}

impl From<leaf_core::Capabilities> for CapabilitiesView {
    fn from(c: leaf_core::Capabilities) -> Self {
        Self {
            bold: c.bold,
            italic: c.italic,
            code: c.code,
            mark: c.mark,
            underline: c.underline,
            strike: c.strike,
            superscript: c.superscript,
            subscript: c.subscript,
            heading: c.heading,
            blockquote: c.blockquote,
            bullet_list: c.bullet_list,
            ordered_list: c.ordered_list,
            task: c.task,
            link: c.link,
            mark_color: c.mark_color,
            image: c.image,
            thematic_break: c.thematic_break,
            footnote: c.footnote,
            code_block: c.code_block,
            code_language: c.code_language,
            table: c.table,
            cell_line_break: c.cell_line_break,
            alignment: c.alignment,
            line_spacing: c.line_spacing,
            font_size: c.font_size,
            font_family: c.font_family,
            text_color: c.text_color,
            page_break: c.page_break,
            move_block: c.move_block,
        }
    }
}

/// One formula standing as a picture: what to typeset and where its picture
/// goes. The web peer of [`leaf_core::MathInfo`]. Two shapes:
///
/// - **Inline** (`inline: true`): one row, and on it exactly one run with
///   role `math` whose `src` equals this `src` — a single `∑` standing for
///   the whole formula. The renderer typesets the TeX — through
///   `leaf-math-wasm`'s `typeset_math`, a module it loads on the first
///   formula — and puts the SVG in the run's place as an inline element
///   whose baseline is the picture's (`vertical-align: -depth`), so the text
///   baseline passes through it where the formula's does.
/// - **Block** (`inline: false`): the rows in `[row, row + rows)` are the
///   placeholder, a [`MediaView`]'s shape exactly — the renderer skips them
///   and positions the picture over them, centred.
///
/// A formula on the caret's line is not here: there it is its TeX, drawn as
/// `code` runs between `delimiter` runs, in every markup mode.
#[derive(Serialize, Tsify)]
pub struct MathView {
    /// The first [`DocView::rows`] row of the formula — the row its atom is
    /// on, or the placeholder row of a block.
    row: usize,
    /// How many rows it spans: `1` for an atom; the placeholder and its
    /// fillers for a block, which is `1` until [`LeafDoc::set_math_rows`]
    /// says otherwise.
    rows: usize,
    /// Whether this is an atom in a line of text, or a block of its own.
    inline: bool,
    /// The TeX between the delimiters, verbatim. What `typeset_math` takes.
    tex: String,
    /// Display style (limits above and below, full-height fractions) rather
    /// than text style — a `$$…$$`, inline or not.
    display: bool,
    /// The formula's source start: what the `math` run's `src` carries, and
    /// where a click on the picture lands the caret.
    src: usize,
}

/// A per-formula measured height, the way JS reports one back — the input
/// half of the height loop [`LeafDoc::set_math_rows`] closes. The web
/// renderer positions a block formula's element over the rows it reserved
/// and lets the element be as tall as it is, so it seldom needs this.
#[derive(Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
pub struct MathHeight {
    /// The formula's `tex` as [`MathView`] handed it over.
    tex: String,
    /// How many visual rows the rendered picture needs.
    rows: usize,
}

/// A per-destination measured height, the way JS reports one back — the input
/// half of the height loop [`LeafDoc::set_media_rows`] closes.
#[derive(Deserialize, Tsify)]
#[tsify(from_wasm_abi)]
pub struct MediaHeight {
    /// The media's `destination`, keying it to a [`MediaView`].
    destination: String,
    /// How many visual rows the rendered element needs, measured by the renderer.
    rows: usize,
}

/// A rendered frame: the rows to paint, where the caret sits, and the
/// toolbar state — everything the JS side needs for one repaint, in one object.
///
/// `into_wasm_abi` makes this the *return type* of every view-producing method:
/// the generated `.d.ts` types those methods as `DocView` rather than `any`, so
/// the JS renderer sees the full shape.
///
/// ## Whole or a change
///
/// By default every frame is whole: `rows` is every row of the document, and
/// the frame before it is forgotten. After [`LeafDoc::set_incremental_frames`]
/// the same methods answer with a frame whose `rows` are only the rows that
/// changed since the frame before — a caret move lifts none, a keystroke lifts
/// the row it landed on — and the five fields after `rows` say where they go.
/// Which kind a frame is, it says itself: `basis` is `0` on a whole frame and
/// the frame before's number on a change. A caller applies a change to the
/// frame it holds (see `rows`), and one that holds a frame other than `basis`
/// has lost step and asks [`LeafDoc::view`] for a whole one, which every
/// change after that is against. The rest of the frame — the caret, the
/// selection, the toolbar state, `tables`, `directives`, `media` and `math` —
/// is complete on every frame of either kind.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct DocView {
    /// The rows to paint: every row of the document on a whole frame, and on
    /// a change (`basis != 0`) the rows that replace `replaced` of the frame
    /// before's from `row_start`, after which the rows that follow are the
    /// frame before's with `src_shift` added to every `Run::src` they carry.
    /// So a frame is applied as: splice `rows` over `row_start..row_start +
    /// replaced`, then move the offsets of the rows after the splice. An
    /// empty `rows` with `replaced == 0` is a frame that changed no row.
    rows: Vec<Row>,
    /// This frame's number: one more than the frame before's, from `1` at
    /// the first. What a change names as its `basis`.
    frame: u32,
    /// The number of the frame these `rows` are a change against, or `0` when
    /// they are the whole document. A change is applied only to a copy of
    /// exactly that frame; see the type's docs.
    basis: u32,
    /// Where `rows` begin, as an index into the document's rows — the
    /// frame before's and, since a change never moves the rows above it, this
    /// one's. `0` on a whole frame.
    row_start: usize,
    /// How many of the frame before's rows, from `row_start`, `rows` replace.
    /// `0` on a whole frame.
    replaced: usize,
    /// How many rows the document has once this frame is applied — what
    /// `rows.length` is on a whole frame, so a caller sizing a scroll view
    /// reads this on either kind.
    row_count: usize,
    /// The byte offset every `Run::src` in a row *after* the replaced span
    /// moved by — an edit shifts the source of everything below it, and
    /// those rows are otherwise the frame before's, so they are kept and
    /// moved rather than lifted. `0` on a whole frame, and on a change that
    /// moved nothing. A 32-bit number rather than the 64-bit one serde would
    /// hand JS as a `BigInt`.
    src_shift: i32,
    /// Tables described structurally, for the proportional renderer that draws
    /// its own grid instead of painting the box-glyph rows. Empty in the source
    /// view. Each names the span of the *document's* rows its picture
    /// occupies, to be skipped — an index into a whole frame's `rows`, and
    /// into the rows a change has been applied to. These lists (and
    /// `directives`, `media`, `math`) are small and ride every frame
    /// complete, so no frame has to say whether they changed.
    tables: Vec<TableView>,
    /// Leaf directives (`::name{…}`) described structurally, for a renderer that
    /// paints what the host app's vocabulary makes of them instead of the `⧉`
    /// placeholder row. Empty in the source view, where the directive is the
    /// literal text the caret is editing.
    directives: Vec<DirectiveView>,
    /// The caret's row: an index into [`Self::rows`].
    caret_row: usize,
    /// The caret's display *column* within its row — core's grid position. Kept
    /// for callers reasoning in columns; a proportional DOM renderer wants
    /// [`Self::caret_ch`] instead.
    caret_col: usize,
    /// The caret's offset within its row's text in **UTF-16 code units** — what a
    /// DOM `Range` counts to. This is `caret_col` mapped through the row's
    /// grapheme widths, so it lands the caret correctly past wide glyphs (CJK,
    /// emoji) where a column and a character index diverge. The renderer builds a
    /// collapsed `Range` at this offset to place the caret.
    caret_ch: usize,
    /// Whether a (non-empty) selection is active. When true, the renderer paints
    /// the browser's native selection over `[anchor_row/anchor_ch, caret]` and
    /// hides its own caret; when false, only the caret shows.
    has_selection: bool,
    /// The selection's *fixed* end (the caret is the moving end), as a row and a
    /// UTF-16 offset — so the renderer can restore a native selection with the
    /// same direction the model has, and a following Shift-motion extends from
    /// the right edge. Equal to the caret position when `has_selection` is false.
    anchor_row: usize,
    anchor_ch: usize,
    /// Whether the buffer differs from the last saved bytes — for a "● modified"
    /// affordance.
    dirty: bool,
    /// Whether there is a step to undo, and one to redo — what a toolbar's
    /// history buttons enable by. Both false on a read-only document. Exact:
    /// `can_undo` is true precisely when `undo()` would move the document.
    can_undo: bool,
    can_redo: bool,
    /// `"wysiwyg"` or `"source"`, for a view-toggle affordance.
    view: String,
    /// The heading level at the caret, if any — a toolbar lights H1…H6 from it.
    heading: Option<u32>,
    /// Whether the caret stands in a code block — the toolbar lights its Code
    /// Block button from it. Rides the frame for `heading`'s reason: walking
    /// into a fence changes no mark, so a button asking for itself would never
    /// be told.
    code_block: bool,
    /// Whether the list item at the caret carries a checkbox, and which way it
    /// faces — `Some(true)` ticked, `Some(false)` empty, `None` for a plain
    /// item or no item at all. Rides the frame for `heading`'s reason: stepping
    /// the caret from a bullet into a task item changes no mark, so a button
    /// asking for itself would never be told.
    task: Option<bool>,
    /// The inline marks active at the caret (`bold`, `italic`, `code`, …) — the
    /// toolbar lights the matching buttons, the same state the TUI prints in its
    /// footer.
    active: Vec<String>,
    /// The caret's **source byte offset** — the coordinate a table cell is keyed
    /// by ([`TableCellView::start`]/[`TableCellView::end`]), so a renderer
    /// drawing its own grid can find which cell the caret sits in without the
    /// picture-row indices.
    caret_src: usize,
    /// The destination of the link the caret stands in, or `null` — a toolbar
    /// lights its Link button from it and seeds an edit of that link with it.
    ///
    /// It rides the frame rather than being a query the toolbar makes for itself
    /// because a toolbar only redraws when the *state* changes: walking the
    /// caret out of a link changes no mark, no heading, and no dirty flag, so a
    /// Link button reading this by a call of its own would keep a stale light
    /// on. Same reason `heading` is here and not asked for.
    link: Option<String>,
    /// The colour of the highlight the caret stands in (`"red"`, …), or `null`
    /// — both outside a highlight and inside one that names no colour. Which
    /// swatch a colour control marks as the current one.
    ///
    /// It rides the frame for `link`'s reason, and more sharply: walking from a
    /// red highlight into a blue one changes no mark, no heading, no dirty flag
    /// and no link, so a palette asking for itself would never be told to move.
    ///
    /// The name rather than a code, matching `Run::mark_color` and the
    /// `data-color` the document carries — a web renderer keys a CSS custom
    /// property off exactly this string.
    mark_color: Option<String>,
    /// Every block-level image, video, and audio in the frame, in row order —
    /// the placeholder rows the renderer replaces with real elements. Empty in
    /// the source view, which shows the markup itself and has no placeholders.
    media: Vec<MediaView>,
    /// Every formula standing as a picture — each inline atom and each
    /// display block — for the renderer to typeset and draw in place of the
    /// `math` run or the placeholder rows. Empty in the source view, and
    /// empty of any formula on the caret's line, which is its TeX there.
    math: Vec<MathView>,
    /// Which built map these rows came from — `leaf_core::Doc::visual_key`,
    /// spelled as a string the renderer compares and never reads. Two frames
    /// with the same key have the same rows, so a renderer that repaints only
    /// when this moves repaints exactly when the rows did.
    ///
    /// What it is for: a caret move without an edit still changes the map
    /// when it crosses onto or off a line that reveals — every line under
    /// `MarkupMode::Full`, a formula's line in every mode — and a renderer
    /// that mirrors the native selection into core without repainting would
    /// keep showing the picture the caret is now standing in the source of.
    map_key: String,
}

/// Whether `b` is `a` with every `Run::src` moved by `shift` — the row
/// equality [`leaf_core::row_delta`] matches the frame before's suffix on. Every
/// field is named so that a field added to [`Row`] or [`Run`] is a compile
/// error here rather than a row the delta silently ignores.
fn same_row_shifted(a: &Row, b: &Row, shift: i64) -> bool {
    let Row {
        runs,
        decoration,
        code,
        code_lang,
        directive,
        directive_label,
        boundary,
        heading,
        align,
        line_height,
    } = a;
    *decoration == b.decoration
        && *code == b.code
        && *code_lang == b.code_lang
        && *directive == b.directive
        && *directive_label == b.directive_label
        && *boundary == b.boundary
        && *heading == b.heading
        && *align == b.align
        && *line_height == b.line_height
        && runs.len() == b.runs.len()
        && runs
            .iter()
            .zip(&b.runs)
            .all(|(x, y)| same_run_shifted(x, y, shift))
}

fn same_run_shifted(a: &Run, b: &Run, shift: i64) -> bool {
    let Run {
        text,
        role,
        bold,
        italic,
        underline,
        strike,
        sup,
        sub,
        src,
        sel,
        hl,
        hl_color,
        mark_color,
        token,
        size,
        font,
        text_color,
    } = a;
    *src as i64 + shift == b.src as i64
        && *text == b.text
        && *role == b.role
        && *bold == b.bold
        && *italic == b.italic
        && *underline == b.underline
        && *strike == b.strike
        && *sup == b.sup
        && *sub == b.sub
        && *sel == b.sel
        && *hl == b.hl
        && *hl_color == b.hl_color
        && *mark_color == b.mark_color
        && *token == b.token
        && *size == b.size
        && *font == b.font
        && *text_color == b.text_color
}

/// The shift `b`'s offsets stand at from `a`'s, read off the first run — or
/// `None` for a row with no run to read it off, which matches at any shift.
fn shift_between_rows(a: &Row, b: &Row) -> Option<i64> {
    Some(b.runs.first()?.src as i64 - a.runs.first()?.src as i64)
}

/// The UTF-16 offset into `text` of display column `col` — the position a DOM
/// `Range` counts to. Walks grapheme clusters exactly as core measures columns
/// ([`text_width`] per cluster), so a wide cluster advances the column by its
/// cells while the offset advances by its UTF-16 length; the two coincide only
/// on plain ASCII.
///
/// A `col` falling *inside* a wide cluster resolves past it, to the boundary
/// after — the loop consumes a cluster whole or not at all. Core never asks for
/// one: a caret column is always a cluster start.
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
/// in `text` — the inverse of [`col_to_utf16`], turning a DOM click position
/// back into core's column. Core then clamps the column to a real caret stop.
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
/// id (`h1`…`h6`) so a single CSS rule per level styles it.
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
        // renders to, or a display block's placeholder label. The renderer
        // pairs a `math` run with its [`MathView`] by `src` and draws the
        // typeset picture in its place — see [`DocView::math`].
        Role::Math => "math".into(),
        Role::Delimiter => "delimiter".into(),
    }
}

/// The renderer class id for a block class — the vocabulary [`BoundaryView`] is
/// spelled in. `other` is the honest answer for a kind core doesn't separate
/// out, so a new one is additive: nothing has to change until it wants to space
/// that kind differently.
fn class_name(c: BlockClass) -> String {
    match c {
        BlockClass::Paragraph => "paragraph",
        BlockClass::Heading => "heading",
        BlockClass::List => "list",
        BlockClass::ListItem => "list-item",
        BlockClass::Quote => "quote",
        BlockClass::Code => "code",
        BlockClass::Table => "table",
        BlockClass::Media => "media",
        BlockClass::Math => "math",
        BlockClass::Directive => "directive",
        BlockClass::Rule => "rule",
        BlockClass::Footnote => "footnote",
        BlockClass::Other => "other",
    }
    .to_string()
}

/// The toolbar id for an inline mark — kept in sync with the JS button ids.
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

/// A live leaf document bound for the browser: `leaf_core::Doc` plus the wrap
/// width the current viewport implies. Constructed from an in-memory string and
/// driven entirely through method calls — there is no filesystem behind it.
#[wasm_bindgen]
pub struct LeafDoc {
    doc: Doc,
    /// The wrap width in columns, from the viewport. `build_visual` caches on
    /// `(revision, width)`, so re-syncing when neither moved is free.
    width: usize,
    /// The page's colour scheme, which a `<picture>`'s `prefers-color-scheme`
    /// `<source>`s are matched against when resolving a block image's URL. Core
    /// has no theme of its own, so this is the browser's answer on its behalf;
    /// defaults to [`ColorScheme::Light`], the web's own default, until the host
    /// calls [`set_color_scheme`](LeafDoc::set_color_scheme).
    scheme: ColorScheme,
    /// Whether a frame is the change since the frame before rather than the
    /// whole document — see [`LeafDoc::set_incremental_frames`].
    incremental: bool,
    /// The rows of the last frame handed out, kept only while `incremental`
    /// so the next frame can be the difference from them; `None` makes the
    /// next frame whole. Whenever it is `Some`, it is frame `frame`'s rows.
    last_rows: Option<Vec<Row>>,
    /// The number of the last frame handed out — `0` before the first.
    frame: u32,
}

#[wasm_bindgen]
impl LeafDoc {
    /// Parse `source` as `format` (`"markdown"`/`"md"`, `"djot"`/`"dj"`,
    /// `"html"`, `"xml"`) into a live, untitled document.
    #[wasm_bindgen(constructor)]
    pub fn new(source: &str, format: &str) -> Result<LeafDoc, JsValue> {
        console_error_panic_hook::set_once();
        let format = match format.to_ascii_lowercase().as_str() {
            "markdown" | "md" => Format::Markdown,
            "djot" | "dj" => Format::Djot,
            "html" | "htm" => Format::Html,
            "xml" => Format::Xml,
            other => return Err(JsValue::from_str(&format!("unknown format: {other}"))),
        };
        let doc = Doc::from_source(source.to_string(), format)
            .map_err(|e| JsValue::from_str(&format!("{e}")))?;
        Ok(LeafDoc {
            doc,
            width: 80,
            scheme: ColorScheme::Light,
            incremental: false,
            last_rows: None,
            frame: 0,
        })
    }

    /// Rebuild the visual map at the current width. Cheap (cached) when nothing
    /// changed; the guard that lets every movement/click method assume a fresh
    /// grid regardless of the order JS calls them in.
    fn sync(&mut self) {
        self.doc.build_visual(self.width);
        // The source view's styling, keyed on the revision alone — a no-op on
        // every call that isn't the first after an edit, like the map above.
        if self.doc.view == View::Source {
            self.doc.build_source();
        }
    }

    /// The plain text of visual row `row` in the active view — the same string
    /// the renderer concatenates its runs into. It backs the column⇄UTF-16
    /// mapping ([`col_to_utf16`]/[`utf16_to_col`]); the two views draw from
    /// different sources (resolved glyphs vs raw source lines), so it branches
    /// the same way [`LeafDoc::view`] does.
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

    /// The `(row, display-column)` a source offset sits at in the active view —
    /// the counterpart to [`Doc::caret_pos`] for an arbitrary offset (the caret
    /// is `caret_pos`, but the selection's anchor needs the same for any offset).
    /// Branches by view exactly as `caret_pos` does.
    fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.pos_of_offset(off),
            View::Source => {
                let s = &self.doc.source;
                let off = off.min(s.len());
                let row = s[..off].bytes().filter(|&b| b == b'\n').count();
                let line_start = s[..off].rfind('\n').map_or(0, |i| i + 1);
                (row, text_width(&s[line_start..off]))
            }
        }
    }

    // ── position mapping (non-mutating; the caret is untouched) ─────────────
    //
    // Each branches by view exactly as [`Self::pos_of_offset`] does, so the
    // WYSIWYG map and the raw-source grid answer in their own coordinates. They
    // back the offset-addressed methods a host needs when it is drawing part of
    // the document somewhere the caret isn't — a footnote peek, a link preview —
    // and must not move the caret to answer.

    /// The byte offset where visual `row` begins in the source view.
    fn source_line_start(&self, row: usize) -> usize {
        self.doc
            .source
            .split('\n')
            .take(row)
            .map(|l| l.len() + 1)
            .sum()
    }

    /// The source offset of display column `col` on visual `row` — the inverse
    /// of [`Self::pos_of_offset`] in column space.
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

    /// The rows a source range covers, both ends inclusive.
    fn row_range_span(&self, start: usize, end: usize) -> (usize, usize) {
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.row_range_for(start..end),
            View::Source => {
                let first = self.pos_of_offset(start).0;
                let last = self.pos_of_offset(end.max(start.saturating_add(1)) - 1).0;
                (first, last.max(first))
            }
        }
    }

    /// Resolve the current document to a whole frame — the first paint, and
    /// the frame a renderer taking changes ([`set_incremental_frames`]) is
    /// brought back into step by: every change after this is against it.
    ///
    /// [`set_incremental_frames`]: Self::set_incremental_frames
    pub fn view(&mut self) -> Result<DocView, JsValue> {
        let v = self.whole();
        self.frame = v.frame;
        self.last_rows = self.incremental.then(|| v.rows.clone());
        Ok(v)
    }

    /// A whole frame of the document as a page shows it: no line revealed,
    /// where [`view`](Self::view) reveals the caret's — its delimiters under
    /// the `"full"` markup mode, and in every mode a formula on it as its TeX.
    /// What a print path renders from, since paper has no caret. See
    /// [`leaf_core::Doc::set_unrevealed`].
    ///
    /// Kept apart from the screen: the screen's map is set aside and put back,
    /// and neither the frame count nor the rows kept for the next change move,
    /// so the frame after this one is still a change from the screen's last.
    pub fn paper_view(&mut self) -> Result<DocView, JsValue> {
        // Bring the screen's map up to date first, so an edit it has not yet
        // been built for is spent on it rather than on the paper's build.
        self.sync();
        self.doc.set_unrevealed(true);
        let v = self.whole();
        self.doc.set_unrevealed(false);
        Ok(v)
    }

    /// Whether the frame every method answers with is the change since the
    /// frame before rather than the whole document — see [`DocView`] for the
    /// shape, and for what a renderer does with one. Off by default, so a
    /// renderer that reads `rows` as the document goes on getting it; one
    /// that keeps its own copy of the rows turns it on once and splices each
    /// frame into that copy, which makes a caret move lift no row across the
    /// boundary and a keystroke lift the row it changed.
    ///
    /// The first frame after turning it on is whole (there is no frame before
    /// to be a change from), and [`view`](Self::view) is whole at any time.
    pub fn set_incremental_frames(&mut self, on: bool) {
        self.incremental = on;
        self.last_rows = None;
    }

    /// The document's rows `from..to`, as the last frame had them — for a
    /// renderer that wants a window of rows back without a whole frame,
    /// having applied every change so far or not. Clamped to the document;
    /// empty when `from >= to`. Costs the rows of the document to build and
    /// the window to lift, so it is the occasional resynchronisation, not the
    /// per-gesture path.
    pub fn rows(&mut self, from: usize, to: usize) -> Vec<Row> {
        let mut rows = self.whole().rows;
        let to = to.min(rows.len());
        let from = from.min(to);
        rows.truncate(to);
        rows.drain(..from);
        rows
    }

    /// The frame every mutating method answers with, so one boundary crossing
    /// both edits and repaints: whole unless the renderer asked for changes,
    /// and then the rows that differ from the frame before's — found by
    /// comparing them in memory, which costs microseconds and lifts nothing —
    /// with the frame before's rows kept for the next one.
    fn frame(&mut self) -> Result<DocView, JsValue> {
        let mut v = self.whole();
        self.frame = v.frame;
        if !self.incremental {
            return Ok(v);
        }
        let Some(old) = self.last_rows.take() else {
            self.last_rows = Some(v.rows.clone());
            return Ok(v);
        };
        let rows = std::mem::take(&mut v.rows);
        let d = leaf_core::row_delta(&old, &rows, same_row_shifted, shift_between_rows);
        v.basis = v.frame - 1;
        v.row_start = d.start;
        v.replaced = d.replaced;
        v.src_shift = d.src_shift as i32;
        v.rows = rows[d.start..d.start + d.len].to_vec();
        self.last_rows = Some(rows);
        Ok(v)
    }

    /// Resolve the current document to a whole frame of style runs, numbered
    /// as the next one. What both [`view`](Self::view) and
    /// [`frame`](Self::frame) start from; neither the frame count nor the
    /// rows kept for the next change are touched here.
    fn whole(&mut self) -> DocView {
        self.sync();

        let (ss, se) = self.doc.selection().unwrap_or((usize::MAX, usize::MAX));

        // The two views speak different grids — the WYSIWYG map's resolved glyphs
        // vs the raw source split on newlines — and `caret_pos` below already
        // branches to match, so the rows must too or the caret lands on the wrong
        // text. See `Doc::caret_pos`.
        let rows = match self.doc.view {
            View::Wysiwyg => wysiwyg_rows(&self.doc.vmap, ss, se, self.doc.highlights()),
            View::Source => source_rows(
                &self.doc.source,
                &self.doc.smap,
                ss,
                se,
                self.doc.highlights(),
            ),
        };

        let (caret_row, caret_col) = self.doc.caret_pos();
        // Map the caret's display column to a UTF-16 text offset so the DOM
        // renderer can place it past wide glyphs (see [`DocView::caret_ch`]).
        let caret_ch = col_to_utf16(&self.row_text(caret_row), caret_col);
        // The selection's fixed (anchor) end, in the same row/UTF-16 terms, so
        // the renderer can mirror it onto the browser's native selection.
        let (has_selection, anchor_row, anchor_ch) = match self.doc.selection() {
            Some(_) => {
                let a = self.doc.anchor.unwrap_or(self.doc.caret);
                let (ar, ac) = self.pos_of_offset(a);
                (true, ar, col_to_utf16(&self.row_text(ar), ac))
            }
            None => (false, caret_row, caret_ch),
        };
        let heading = self.doc.current_heading_level();
        let code_block = self.doc.caret_in_code_block();
        let task = self.doc.task_checked_at_caret();
        // Read before the frame is assembled: it needs `&mut self`, which the
        // struct literal's other fields are already borrowing out of.
        let link = self.doc.link_destination_at_caret();
        let mark_color = self.doc.mark_color_at_caret().map(|c| c.name().to_string());
        let active = self
            .doc
            .active_inline_marks()
            .iter()
            .map(|k| mark_id(k).to_string())
            .collect();

        let row_count = rows.len();
        DocView {
            rows,
            frame: self.frame + 1,
            basis: 0,
            row_start: 0,
            replaced: 0,
            row_count,
            src_shift: 0,
            // Both are structural alternatives to rows the WYSIWYG map drew as a
            // picture; the source view has no picture, only the markup itself.
            tables: match self.doc.view {
                View::Wysiwyg => wysiwyg_tables(&self.doc.vmap, ss, se, self.doc.highlights()),
                View::Source => Vec::new(),
            },
            directives: match self.doc.view {
                View::Wysiwyg => wysiwyg_directives(&self.doc.vmap),
                View::Source => Vec::new(),
            },
            caret_row,
            caret_col,
            caret_ch,
            has_selection,
            anchor_row,
            anchor_ch,
            dirty: self.doc.dirty,
            can_undo: self.doc.can_undo(),
            can_redo: self.doc.can_redo(),
            view: self.doc.view_name().to_string(),
            heading,
            code_block,
            task,
            active,
            caret_src: self.doc.caret,
            link,
            mark_color,
            // Only the WYSIWYG view has placeholder rows to replace; the source
            // view is the markup itself, where a `<video>` tag *is* the content.
            media: match self.doc.view {
                View::Wysiwyg => media_views(&self.doc.vmap, self.scheme),
                View::Source => Vec::new(),
            },
            math: match self.doc.view {
                View::Wysiwyg => math_views(&self.doc.vmap),
                View::Source => Vec::new(),
            },
            map_key: format!("{:?}", self.doc.visual_key()),
        }
    }

    /// Tell core which colour scheme the page is in (`"dark"` / `"light"`), so a
    /// `<picture>`'s `prefers-color-scheme` `<source>`s resolve to the right
    /// banner. Anything unrecognised is treated as light, the web's default.
    ///
    /// Cheap to call on every `matchMedia` change: a repaint at the same scheme
    /// re-resolves to the same URLs, and the elements the renderer already built
    /// are keyed by destination, so nothing is torn down needlessly.
    pub fn set_color_scheme(&mut self, scheme: &str) -> Result<DocView, JsValue> {
        self.scheme = match scheme.to_ascii_lowercase().as_str() {
            "dark" => ColorScheme::Dark,
            _ => ColorScheme::Light,
        };
        self.frame()
    }

    /// Report how many visual rows each block media actually needs, measured
    /// from the real elements the renderer built, keyed by destination.
    ///
    /// Core does no I/O and can't know how tall a picture or a player is, so
    /// this is the only way a placeholder grows past its default single row.
    /// The loop is: paint at the current reservation → measure the elements →
    /// call this → repaint if it changed. Handing over the same measurements
    /// again is a no-op, so a renderer can just report its current state each
    /// frame without checking whether anything moved.
    pub fn set_media_rows(&mut self, heights: Vec<MediaHeight>) -> Result<DocView, JsValue> {
        self.doc.set_media_rows(
            heights
                .into_iter()
                .map(|h| (h.destination, h.rows))
                .collect(),
        );
        self.frame()
    }

    /// Report how many visual rows each display formula needs, keyed by its
    /// TeX as [`MathView`] handed it over — [`set_media_rows`]'s peer.
    ///
    /// [`set_media_rows`]: Self::set_media_rows
    pub fn set_math_rows(&mut self, heights: Vec<MathHeight>) -> Result<DocView, JsValue> {
        self.doc
            .set_math_rows(heights.into_iter().map(|h| (h.tex, h.rows)).collect());
        self.frame()
    }

    /// Say whether the renderer can paint a picture *inside* a line of text.
    /// When it can, an inline formula arrives as one `math` run and a
    /// [`MathView`] with `inline` set, for the renderer to put its typeset
    /// picture in place of; when it cannot, as the code-styled TeX it always
    /// was. Off until called, so a host that has not caught up sees what it
    /// saw.
    pub fn set_inline_pictures(&mut self, on: bool) -> Result<DocView, JsValue> {
        self.doc.set_inline_pictures(on);
        self.frame()
    }

    /// Insert a block-level image, video, or audio at the caret — `kind` is
    /// `"image"`, `"video"`, or `"audio"`. Any selection becomes the alt text.
    /// See [`leaf_core::Doc::insert_media`] for the markup each spells.
    pub fn insert_media(
        &mut self,
        kind: &str,
        destination: &str,
        alt: &str,
    ) -> Result<DocView, JsValue> {
        let kind = match kind.to_ascii_lowercase().as_str() {
            "image" | "img" => MediaKind::Image,
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            other => return Err(JsValue::from_str(&format!("unknown media kind: {other}"))),
        };
        self.doc.insert_media(kind, destination, alt);
        self.frame()
    }

    /// Append media at the end of the document as a block of its own — the
    /// same kinds as `insert_media`. See [`leaf_core::Doc::append_media`].
    pub fn append_media(
        &mut self,
        kind: &str,
        destination: &str,
        alt: &str,
    ) -> Result<DocView, JsValue> {
        let kind = match kind.to_ascii_lowercase().as_str() {
            "image" | "img" => MediaKind::Image,
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            other => return Err(JsValue::from_str(&format!("unknown media kind: {other}"))),
        };
        self.doc.append_media(kind, destination, alt);
        self.frame()
    }

    /// Set the wrap width (in columns) the viewport implies and repaint.
    pub fn set_width(&mut self, cols: usize) -> Result<DocView, JsValue> {
        self.width = cols.max(1);
        self.frame()
    }

    /// The current source text — for a "save" (download / localStorage / PUT) or
    /// a source-view display.
    pub fn source(&self) -> String {
        self.doc.source.clone()
    }

    /// The selected text, if any — for a clipboard copy/cut.
    pub fn selected_text(&self) -> Option<String> {
        self.doc.selected_text().map(str::to_string)
    }

    /// The selection as a quote with up to `context` characters of what
    /// surrounded it, cut from the **source** — the shape a host that cites or
    /// annotates a passage wants, findable in the document again by plain
    /// string search. `None` when nothing is selected. See
    /// `leaf_core::Doc::selection_quote`.
    pub fn selection_quote(&self, context: u32) -> Option<SelectionQuote> {
        self.doc
            .selection_quote(context as usize)
            .map(|q| SelectionQuote {
                exact: q.exact,
                prefix: q.prefix,
                suffix: q.suffix,
                start: q.start,
                end: q.end,
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
        self.doc.counts().into()
    }

    /// The same statistics over the selection alone — `undefined` when nothing
    /// is selected. See `leaf_core::Doc::selection_counts`.
    pub fn selection_counts(&self) -> Option<TextCounts> {
        self.doc.selection_counts().map(Into::into)
    }

    /// Whether the document refuses to change — see `set_read_only`.
    pub fn read_only(&self) -> bool {
        self.doc.read_only()
    }

    /// Turn the read-only gate on or off — a *reading* surface over the same
    /// rendering, selection and navigation the editor has. Enforced in core at
    /// the three doors every mutation goes through, so a host that also quiets
    /// its input chrome is polishing, not protecting.
    pub fn set_read_only(&mut self, on: bool) -> Result<DocView, JsValue> {
        self.doc.set_read_only(on);
        self.frame()
    }

    /// Replace the host-painted source ranges wholesale and repaint — see
    /// `leaf_core::Doc::set_highlights` for why it is a replace. Takes
    /// `[start, end, id, color?]` tuples as a JS array of objects.
    pub fn set_highlights(&mut self, highlights: Vec<HighlightIn>) -> Result<DocView, JsValue> {
        self.doc.set_highlights(
            highlights
                .into_iter()
                .map(|h| CoreHighlight {
                    start: h.start,
                    end: h.end,
                    id: h.id,
                    color: h.color,
                    marker: h.marker,
                })
                .collect(),
        );
        self.frame()
    }

    /// The id of the highlight covering source `offset`, if one does — what a
    /// frontend asks when the reader activates a spot on the page.
    pub fn highlight_at(&self, offset: usize) -> Option<String> {
        self.doc.highlight_at(offset).map(|h| h.id.clone())
    }

    /// The host-painted ranges as last set, sorted by start — what a frontend
    /// walks to lay out margin markers.
    pub fn highlights(&self) -> Vec<HighlightOut> {
        self.doc
            .highlights()
            .iter()
            .map(|h| HighlightOut {
                start: h.start,
                end: h.end,
                id: h.id.clone(),
                color: h.color.clone(),
                marker: h.marker.clone(),
            })
            .collect()
    }

    /// Which formatting controls this document's format can actually spell —
    /// one flag per toolbar button.
    ///
    /// Read once when a document opens: the answer depends only on the format,
    /// so no edit can change it. Every gesture refuses on its own regardless, so
    /// ignoring this stays correct and merely offers buttons that do nothing but
    /// set a status message.
    ///
    /// Don't collapse it to one flag. An HTML document takes ⌘B, ⌘I and inline
    /// code — its marks are a tag pair — while refusing every heading, list,
    /// quote and link, and Markdown refuses the underline djot spells.
    pub fn capabilities(&self) -> CapabilitiesView {
        self.doc.capabilities().into()
    }

    /// Whether this document's format offers *any* door in — `false` only for a
    /// wholly parse-only one (XML), where the formatting section can be hidden
    /// outright. For anything finer, including whether to dim an individual
    /// button, use [`LeafDoc::capabilities`].
    pub fn authorable(&self) -> bool {
        self.doc.authorable()
    }

    /// Mark the buffer saved after the host persisted [`LeafDoc::source`] its own
    /// way — clears the dirty flag without touching a filesystem.
    pub fn mark_saved(&mut self) -> Result<DocView, JsValue> {
        self.doc.mark_saved();
        self.frame()
    }

    // ── text input ──────────────────────────────────────────────────────────

    pub fn insert(&mut self, text: &str) -> Result<DocView, JsValue> {
        self.doc.insert(text);
        self.frame()
    }

    pub fn paste(&mut self, text: &str) -> Result<DocView, JsValue> {
        self.doc.paste(text);
        self.frame()
    }

    pub fn newline(&mut self) -> Result<DocView, JsValue> {
        self.doc.newline();
        self.frame()
    }

    /// Tab: indent the caret's line (or the selected lines) one level, nesting a
    /// list item under its sibling.
    pub fn indent(&mut self) -> Result<DocView, JsValue> {
        self.doc.indent();
        self.frame()
    }

    /// Shift+Tab: take one indent level back off the caret's line (or the
    /// selected lines), unnesting a list item.
    pub fn outdent(&mut self) -> Result<DocView, JsValue> {
        self.doc.outdent();
        self.frame()
    }

    pub fn backspace(&mut self) -> Result<DocView, JsValue> {
        self.doc.backspace();
        self.frame()
    }

    pub fn delete_forward(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_forward();
        self.frame()
    }

    pub fn delete_word_back(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_word_back();
        self.frame()
    }

    pub fn delete_word_forward(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_word_forward();
        self.frame()
    }

    // ── caret movement ──────────────────────────────────────────────────────
    // Each syncs the grid first (movement reads the stop table / column layout),
    // moves, then repaints.

    pub fn move_left(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_left(extend);
        self.frame()
    }

    pub fn move_right(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_right(extend);
        self.frame()
    }

    pub fn move_up(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_up(extend);
        self.frame()
    }

    pub fn move_down(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_down(extend);
        self.frame()
    }

    pub fn move_word_left(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_word_left(extend);
        self.frame()
    }

    pub fn move_word_right(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_word_right(extend);
        self.frame()
    }

    pub fn move_home(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_home(extend);
        self.frame()
    }

    pub fn move_end(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_end(extend);
        self.frame()
    }

    pub fn move_doc_start(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_doc_start(extend);
        self.frame()
    }

    pub fn move_doc_end(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_doc_end(extend);
        self.frame()
    }

    pub fn select_all(&mut self) -> Result<DocView, JsValue> {
        self.doc.select_all();
        self.frame()
    }

    /// Place the caret from a click, in core's column grid: `row` indexes the
    /// visual [`Row`]s and `col` is the glyph column within it. A proportional
    /// renderer derives them by hit-testing — `caretRangeFromPoint` gives the DOM
    /// node+offset under the pointer, which maps to a row and a column count. Core
    /// clamps both to real caret stops.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.click(row, col, extend);
        self.frame()
    }

    /// Place the caret from a click whose horizontal position is a **UTF-16
    /// offset** into the visual row's text — what a DOM `Range` hands back
    /// (`range.toString().length`). It's converted to core's display column
    /// (they differ by a cell per wide glyph) before clicking, so a proportional
    /// renderer never has to reason about column widths itself. This is the
    /// hit-test counterpart of [`DocView::caret_ch`]; prefer it over [`click`].
    pub fn click_ch(&mut self, row: usize, ch: usize, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.doc.click(row, col, extend);
        self.frame()
    }

    /// A click in the blank space under the last row: the caret goes onto an
    /// empty paragraph under the last block, opening one if the document does
    /// not end with one, wherever the pointer was horizontally. The renderer
    /// decides "under" — the point is below every block it laid out. See
    /// `leaf_core::Doc::click_past_end`.
    pub fn click_past_end(&mut self) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.click_past_end();
        self.frame()
    }

    /// The source offset under a click at row `row`, `ch` UTF-16 units in — the
    /// same resolution [`click_ch`] does, but returning the offset instead of
    /// moving the caret. It's what the double/triple-click selectors below anchor
    /// on, and it lets a host implement its own gestures (a context menu placing
    /// the caret, say) without a second boundary crossing.
    ///
    /// Resolved against the map as it stands, without moving the caret first:
    /// moving it could rebuild the map under the second of two ends resolved
    /// in a row. Under `MarkupMode::Full`, and for a formula in every mode, the
    /// map is a function of the caret's line, so a click that lands on such a
    /// line changes the row's text — `say ∑ here` becomes `say $x$ here` —
    /// and a `ch` read off the old row would then name the wrong glyph in the
    /// new one. `click` snaps the answer onto a caret stop, so it is not lost
    /// either: `offset_of_pos` never returns anything else.
    fn offset_at(&mut self, row: usize, ch: usize) -> usize {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.offset_of_col(row, col)
    }

    /// Select the word under a click (row, `ch`) — the double-click gesture.
    /// Core reads the word from the source around that offset.
    pub fn select_word_ch(&mut self, row: usize, ch: usize) -> Result<DocView, JsValue> {
        let off = self.offset_at(row, ch);
        self.doc.select_word_at(off);
        self.frame()
    }

    /// Select the whole logical text block under a click (row, `ch`) — the
    /// triple-click gesture. Core reads the paragraph/heading span from the AST,
    /// so it grabs the entire block even where it soft-wraps across visual rows.
    pub fn select_block_ch(&mut self, row: usize, ch: usize) -> Result<DocView, JsValue> {
        let off = self.offset_at(row, ch);
        self.doc.select_block_at(off);
        self.frame()
    }

    /// Mirror a native browser selection into the model: `[anchor, focus]` given
    /// as row + UTF-16 offset pairs (a DOM `Range`'s ends). Each is resolved to a
    /// source offset the way a click is, then set as the selection's fixed and
    /// moving ends — so `selectionchange` can keep core in step with the
    /// selection the browser drew. A collapsed range (`anchor == focus`) just
    /// places the caret.
    pub fn set_selection(
        &mut self,
        anchor_row: usize,
        anchor_ch: usize,
        focus_row: usize,
        focus_ch: usize,
    ) -> Result<DocView, JsValue> {
        let anchor = self.offset_at(anchor_row, anchor_ch);
        let focus = self.offset_at(focus_row, focus_ch);
        self.doc.place_caret(anchor, false);
        if anchor != focus {
            self.doc.place_caret(focus, true);
        }
        self.frame()
    }

    // ── rich clipboard (mirrors leaf-tui / leaf-gpui) ────────────────────────

    /// The current selection rendered to HTML by twig — the rich flavor a copy
    /// writes alongside the plain [`LeafDoc::selected_text`], so pasting into a
    /// word processor keeps the formatting. `None` when nothing is selected.
    pub fn selection_html(&mut self) -> Option<String> {
        self.doc.selection_html()
    }

    /// Paste, preferring the clipboard's rich (`text/html`) flavor: twig parses
    /// `html` into the document's own markup and inserts it. Falls back to the
    /// plain `text` when there's no HTML or it doesn't parse — the same
    /// html-then-plain order the TUI and gpui frontends use.
    pub fn paste_rich(&mut self, html: Option<String>, text: &str) -> Result<DocView, JsValue> {
        let took = html.as_deref().is_some_and(|h| self.doc.paste_html(h));
        if !took {
            self.doc.paste(text);
        }
        self.frame()
    }

    // ── formatting commands (mirror leaf-gpui's EditorCommand) ───────────────

    pub fn toggle_bold(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Strong);
        self.frame()
    }

    pub fn toggle_italic(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Emph);
        self.frame()
    }

    pub fn toggle_code(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Verbatim);
        self.frame()
    }

    pub fn toggle_mark(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Mark);
        self.frame()
    }

    /// Whether the caret stands in a highlight — what a colour control enables
    /// itself by, since a colour is a property of a highlight that already
    /// exists. The caret-side half of `capabilities().markColor`.
    pub fn caret_in_mark(&mut self) -> bool {
        self.doc.caret_in_mark()
    }

    /// Colour the highlight at the caret (`"red"`, `"orange"`, `"yellow"`,
    /// `"green"`, `"blue"`, `"purple"`, `"brown"`), or clear its colour with
    /// `null`/`undefined`.
    ///
    /// The name a `mark` node's `data-color` carries and `Run::mark_color`
    /// reports, so a renderer that already styles the colours has the vocabulary
    /// in hand. A name outside the palette is an error rather than a silent
    /// clearing — the two arguments differ by one typo and mean opposite things.
    ///
    /// Markdown only (djot spells the highlight and no colour on it), and only
    /// where there is a highlight to colour: this does not make one. A coloured
    /// highlight from bare text is `toggleMark` and then this.
    pub fn set_mark_color(&mut self, color: Option<String>) -> Result<DocView, JsValue> {
        self.doc.set_mark_color(mark_color(color.as_deref())?);
        self.frame()
    }

    /// One press of a colour swatch: colour the highlight at the caret, or —
    /// over a selection that isn't highlighted yet — highlight it and colour it,
    /// as **one** undo step.
    ///
    /// `setMarkColor` is the exact gesture; this is the compound a toolbar
    /// presses, and it lives in core so every frontend answers "what does a
    /// swatch mean over plain text" the same way. `null` clears the colour, and
    /// over an unhighlighted selection means simply "highlight this".
    pub fn highlight(&mut self, color: Option<String>) -> Result<DocView, JsValue> {
        self.doc.highlight(mark_color(color.as_deref())?);
        self.frame()
    }

    pub fn toggle_underline(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Insert);
        self.frame()
    }

    pub fn toggle_strike(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Delete);
        self.frame()
    }

    pub fn set_paragraph(&mut self) -> Result<DocView, JsValue> {
        self.doc.set_block(BlockKind::Paragraph);
        self.frame()
    }

    /// Toggle the current block to a heading of `level` (1–6); toggling the
    /// active level off returns it to a paragraph, per core.
    pub fn set_heading(&mut self, level: u32) -> Result<DocView, JsValue> {
        self.doc.toggle_heading(level);
        self.frame()
    }

    pub fn toggle_blockquote(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_blockquote();
        self.frame()
    }

    /// Toggle a fenced code block over the selection or the block at the caret;
    /// on a blank line, open an empty one with the caret inside. Gate on
    /// `capabilities().code_block`; light from the frame's `code_block`.
    pub fn toggle_code_block(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_code_block();
        self.frame()
    }

    pub fn toggle_list(&mut self, ordered: bool) -> Result<DocView, JsValue> {
        self.doc.toggle_list(ordered);
        self.frame()
    }

    /// Tick or untick the task item at the caret.
    pub fn toggle_task_checked(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_task_checked();
        self.frame()
    }

    /// Tick or untick the task item covering `offset` — a click on a rendered
    /// checkbox, which leaves the caret where it was.
    pub fn toggle_task_at(&mut self, offset: usize) -> Result<DocView, JsValue> {
        self.doc.toggle_task_at(offset);
        self.frame()
    }

    /// Give the list item at the caret a checkbox, or take its checkbox away.
    pub fn toggle_task_item(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_task_item();
        self.frame()
    }

    /// Whether the item at the caret has a box, and which way it faces.
    pub fn task_checked_at_caret(&mut self) -> Option<bool> {
        self.doc.task_checked_at_caret()
    }

    // ── the presentation vocabulary ──────────────────────────────────────────
    //
    // Six gestures and five queries over a document's *presentation*: alignment
    // and line spacing, which are the block's and ride [`Row::align`] and
    // [`Row::line_height`], and size, face and colour, which are the run's and
    // ride [`Run::size`], [`Run::font`] and [`Run::text_color`]. Each value is
    // a **name** a stylesheet can select on — `center`, `1.5`, `large`,
    // `serif`, `red` — or, for the four properties whose vocabulary is open,
    // the exact value the author asked for: `14pt`, `Garamond`, `#c03030`,
    // `1.3`. A name outlives the theme it was written under and a value does
    // not, which is the trade the author takes knowingly; `presentation.css` is
    // the rule per name, and a value is an inline style the renderer sets
    // beside the attribute. See `docs/proposals/exact-presentation-values.md`.
    //
    // Each gesture edits one attribute key and keeps the rest, so a document
    // from elsewhere passes through the editor unharmed, and `null` clears the
    // key. A name outside a vocabulary is an error rather than a silent
    // clearing, exactly as it is for `set_mark_color`: the two arguments differ
    // by one typo and mean opposite things. Enable each control by its
    // `capabilities()` flag and light it by the query beside it.

    /// Align the caret's block (`"center"`, `"right"`, `"justify"`), or return
    /// it to the theme's default with `null`/`undefined`.
    ///
    /// A block property, so it is the caret's *block* whatever is selected: a
    /// line belongs to a block, and "centre this" with three words selected
    /// means the paragraph, not the words. Other `class` tokens on the block are
    /// kept. There is no `"left"` — absence is left.
    pub fn set_alignment(&mut self, align: Option<String>) -> Result<DocView, JsValue> {
        let align = alignment(align.as_deref())?;
        self.doc.set_alignment(align);
        self.frame()
    }

    /// Set the line spacing of the caret's block — one of the menu's three
    /// (`"1.15"`, `"1.5"`, `"2"`) or any other positive decimal (`"1.3"`) — or
    /// return it to the theme's with `null`. `set_alignment`'s peer in every
    /// respect but the key.
    ///
    /// A decimal that spells one of the three *is* that name, and `"1"` is
    /// single spacing, which is absence and clears the key rather than writing
    /// a value that says nothing.
    pub fn set_line_spacing(&mut self, spacing: Option<String>) -> Result<DocView, JsValue> {
        let spacing = line_spacing(spacing.as_deref())?;
        self.doc.set_line_spacing(spacing);
        self.frame()
    }

    /// Set the size of the selected run, or of the caret's whole block when
    /// nothing is selected — CSS's absolute-size keywords (`"xx-small"` …
    /// `"xxx-large"`) or a point size (`"14pt"`, `"13.5pt"`), with `null` for
    /// the theme's own size. `"medium"` is not a value: `medium` is absence,
    /// and so is a point size a sheet of paper could not hold.
    ///
    /// Size, face and colour are the *run's*, and the block's when no run is
    /// chosen — so "make this paragraph larger" is a press with the caret in it
    /// rather than a select-all first. With a selection the range is wrapped in
    /// an attributed span, or the span it already lies in is re-styled, never
    /// nested.
    pub fn set_font_size(&mut self, size: Option<String>) -> Result<DocView, JsValue> {
        let size = font_size(size.as_deref())?;
        self.doc.set_font_size(size);
        self.frame()
    }

    /// Set the face of the selected run, or of the caret's whole block — one of
    /// CSS's four generics (`"serif"`, `"sans-serif"`, `"monospace"`,
    /// `"cursive"`), or a family name (`"Garamond"`), or `null` for the theme's
    /// body face. `set_font_size`'s peer.
    ///
    /// A generic opens on every machine; a family name renders in the fallback
    /// everywhere it is not installed, which is the cost the author takes
    /// knowingly. A name that spells a generic is read as that generic.
    pub fn set_font_family(&mut self, font: Option<String>) -> Result<DocView, JsValue> {
        let font = font_family(font.as_deref())?;
        self.doc.set_font_family(font);
        self.frame()
    }

    /// Set the *text* colour of the selected run, or of the caret's whole block
    /// — the same seven names `set_mark_color` takes, or a hex triple
    /// (`"#c03030"`, or the `"#f00"` shorthand), or `null` for the theme's.
    ///
    /// `set_font_size`'s peer, and **not** `set_mark_color`: that one colours a
    /// highlight's background and needs a highlight to colour, this one paints
    /// the letters and needs nothing. They share the vocabulary on purpose, so a
    /// page with a `--leaf-red` for a highlight has it for text too.
    pub fn set_text_color(&mut self, color: Option<String>) -> Result<DocView, JsValue> {
        let color = text_color(color.as_deref())?;
        self.doc.set_text_color(color);
        self.frame()
    }

    /// Insert a page break at the caret — a leaf directive with no label, placed
    /// exactly as `insert_thematic_break` places a rule, a selection replaced by
    /// it and a bare paragraph parted at the caret first.
    ///
    /// A renderer that paginates opens a page at the row's directive mark; one
    /// that scrolls draws a dashed rule where the `⧉ page-break` placeholder row
    /// is.
    pub fn insert_page_break(&mut self) -> Result<DocView, JsValue> {
        self.doc.insert_page_break();
        self.frame()
    }

    /// The alignment in force at the caret, or `undefined` for the theme's
    /// default — which segment of an alignment control is lit.
    ///
    /// Read off the nearest node that names one: the caret's block, and the
    /// `div`s around it after that, so the control follows the caret into a
    /// centred `<div>`.
    pub fn alignment_at_caret(&mut self) -> Option<String> {
        self.doc.alignment_at_caret().map(|a| a.name().to_string())
    }

    /// The line spacing in force at the caret, or `undefined` for the theme's
    /// own — which entry a spacing menu shows ticked. `alignment_at_caret`'s
    /// peer, and the token is the canonical one, so what a menu ticks is what
    /// a gesture wrote.
    pub fn line_spacing_at_caret(&mut self) -> Option<String> {
        self.doc
            .line_spacing_at_caret()
            .map(|l| l.name().to_string())
    }

    /// The size in force at the caret, or `undefined` for the theme's own —
    /// which entry a size menu shows ticked, or the exact size it shows as a
    /// row of its own. Run-level, so the chain starts one node deeper: the
    /// attributed span the caret stands in, then its block, then the `div`s
    /// around it, the nearest winning.
    pub fn font_size_at_caret(&mut self) -> Option<String> {
        self.doc.font_size_at_caret().map(|s| s.name().to_string())
    }

    /// The face in force at the caret, or `undefined` for the theme's body face.
    /// `font_size_at_caret`'s peer.
    pub fn font_family_at_caret(&mut self) -> Option<String> {
        self.doc
            .font_family_at_caret()
            .map(|f| f.name().to_string())
    }

    /// The *text* colour in force at the caret, or `undefined` for the theme's —
    /// which swatch a text-colour control marks as the current one.
    /// `font_size_at_caret`'s peer, and not [`DocView::mark_color`], which reads
    /// a highlight's background off a `mark` node the caret is standing in.
    pub fn text_color_at_caret(&mut self) -> Option<String> {
        self.doc.text_color_at_caret().map(|c| c.name().to_string())
    }

    pub fn insert_link(&mut self, destination: &str) -> Result<DocView, JsValue> {
        self.doc.insert_link(destination);
        self.frame()
    }

    pub fn undo(&mut self) -> Result<DocView, JsValue> {
        self.doc.undo();
        self.frame()
    }

    pub fn redo(&mut self) -> Result<DocView, JsValue> {
        self.doc.redo();
        self.frame()
    }

    /// Open an undo group: every edit until the matching `end_undo_group`
    /// undoes and redoes as one step — a Replace All, say. Groups nest; an
    /// undo or redo closes any that is open.
    pub fn begin_undo_group(&mut self) {
        self.doc.begin_undo_group();
    }

    /// Close the group `begin_undo_group` opened; a no-op when none is open.
    pub fn end_undo_group(&mut self) {
        self.doc.end_undo_group();
    }

    /// Switch between the rendered WYSIWYG surface and the raw source.
    pub fn toggle_view(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_view();
        self.frame()
    }

    /// The current markup-exposure preference as `"none"`, `"shortcuts"` or
    /// `"full"`.
    pub fn markup_mode(&self) -> String {
        match self.doc.markup_mode() {
            CoreMarkupMode::None => "none",
            CoreMarkupMode::Shortcuts => "shortcuts",
            CoreMarkupMode::Full => "full",
        }
        .to_string()
    }

    /// Set the markup-exposure preference from `"none"` / `"shortcuts"` /
    /// `"full"` (an unknown value is ignored). Returns a fresh view to repaint,
    /// which under `"full"` is the first one showing the caret's line raw. The
    /// web demo defaults to `"none"`, the clean surface.
    pub fn set_markup_mode(&mut self, mode: &str) -> Result<DocView, JsValue> {
        match mode {
            "none" => self.doc.set_markup_mode(CoreMarkupMode::None),
            "shortcuts" => self.doc.set_markup_mode(CoreMarkupMode::Shortcuts),
            "full" => self.doc.set_markup_mode(CoreMarkupMode::Full),
            _ => {}
        }
        self.frame()
    }

    /// The current soft-break flow preference as `"fold"` or `"preserve"`.
    pub fn line_flow(&self) -> String {
        match self.doc.line_flow() {
            CoreLineFlow::Fold => "fold",
            CoreLineFlow::Preserve => "preserve",
        }
        .to_string()
    }

    /// Set the soft-break flow preference from `"fold"` / `"preserve"` (an
    /// unknown value is ignored). Returns a fresh view to repaint: `"preserve"`
    /// lays each soft break out as its own row.
    pub fn set_line_flow(&mut self, mode: &str) -> Result<DocView, JsValue> {
        match mode {
            "fold" => self.doc.set_line_flow(CoreLineFlow::Fold),
            "preserve" => self.doc.set_line_flow(CoreLineFlow::Preserve),
            _ => {}
        }
        self.frame()
    }

    // ── offsets ─────────────────────────────────────────────────────────────
    //
    // The frame addresses the document in `(row, ch)`, which is what a renderer
    // paints in. These speak *source byte offsets* instead — the coordinate a
    // table cell, a footnote, and a link destination are all keyed by, and the
    // only one that survives a re-wrap. A host reaches for them when it is
    // pointing at part of the document rather than editing at the caret.

    /// The caret's source byte offset.
    pub fn caret_offset(&self) -> usize {
        self.doc.caret
    }

    /// The selection's fixed end (equals the caret when there is no selection).
    pub fn anchor_offset(&self) -> usize {
        self.doc.anchor.unwrap_or(self.doc.caret)
    }

    /// The last caret stop in the document.
    pub fn doc_end_offset(&mut self) -> usize {
        self.sync();
        let end = self.doc.source.len();
        self.snap_stop(end)
    }

    /// Snap an arbitrary offset to the nearest valid caret stop — a byte in the
    /// middle of a hidden `**` has no caret home of its own.
    pub fn snap_offset(&mut self, off: usize) -> usize {
        self.sync();
        self.snap_stop(off)
    }

    /// Where a source offset sits on screen: its visual `(row, ch)`, `ch` in
    /// UTF-16 units so a DOM `Range` can be built at it directly.
    pub fn pos_for_offset(&mut self, off: usize) -> RowCol {
        self.sync();
        let (row, col) = self.pos_of_offset(off);
        RowCol {
            row,
            ch: col_to_utf16(&self.row_text(row), col),
        }
    }

    /// The source offset at visual `(row, ch)` — the inverse of
    /// [`Self::pos_for_offset`], for hit-testing a DOM point to a position
    /// without moving the caret (which is what [`Self::click_ch`] does).
    pub fn offset_for_pos(&mut self, row: usize, ch: usize) -> usize {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.offset_of_col(row, col)
    }

    /// The rows a source range covers, both ends **inclusive** — for drawing a
    /// block away from where it sits (a footnote peek, a link preview).
    ///
    /// Ask this rather than mapping the two ends through
    /// [`Self::pos_for_offset`] separately: in a table the rows are not in
    /// offset order, so `start`'s row is not always the first.
    pub fn row_range_for(&mut self, start: usize, end: usize) -> RowRange {
        self.sync();
        let (first, last) = self.row_range_span(start, end);
        RowRange { first, last }
    }

    /// Move `off` by `delta` caret stops (negative = left).
    pub fn step_offset(&mut self, off: usize, delta: i32) -> usize {
        self.sync();
        let mut o = self.snap_stop(off);
        if delta >= 0 {
            for _ in 0..delta {
                match self.stop_after(o) {
                    Some(n) => o = n,
                    None => break,
                }
            }
        } else {
            for _ in 0..(-delta) {
                match self.stop_before(o) {
                    Some(p) => o = p,
                    None => break,
                }
            }
        }
        o
    }

    /// How many caret stops separate two offsets, signed by direction.
    pub fn distance_offset(&mut self, from: usize, to: usize) -> i32 {
        self.sync();
        let (mut a, b, sign) = if from <= to {
            (from, to, 1i32)
        } else {
            (to, from, -1i32)
        };
        a = self.snap_stop(a);
        let mut n = 0i32;
        while a < b {
            match self.stop_after(a) {
                Some(x) => {
                    a = x;
                    n += 1;
                }
                None => break,
            }
        }
        n * sign
    }

    /// The offset one visual row above or below `off`, keeping its column, or
    /// `undefined` at the document's edge.
    pub fn vertical_offset(&mut self, off: usize, down: bool) -> Option<usize> {
        self.sync();
        let (row, col) = self.pos_of_offset(off);
        let target = if down {
            self.nav_below(row)
        } else {
            self.nav_above(row)
        };
        target.map(|r| self.offset_of_col(r, col))
    }

    /// The UTF-16 index at which source offset `off` sits in the visible text
    /// (`text_in_range(0, doc_end_offset())`) — the unit a DOM `Range` and the
    /// Apple text systems count in. See `leaf-ffi`'s method of the same name.
    pub fn utf16_index_for_offset(&mut self, off: usize) -> usize {
        self.sync();
        let off = off.min(self.doc.source.len());
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.visible_utf16_len(0, off),
            View::Source => {
                let off = self.snap_stop(off);
                self.doc.source[..off].encode_utf16().count()
            }
        }
    }

    /// `utf16_index_for_offset` over many offsets in one crossing — the index
    /// of each, in the order given. See `leaf-ffi`'s method of the same name.
    pub fn utf16_indices_for_offsets(&mut self, offs: Vec<u32>) -> Vec<u32> {
        offs.into_iter()
            .map(|off| self.utf16_index_for_offset(off as usize) as u32)
            .collect()
    }

    /// The inverse of `utf16_index_for_offset`: the source offset (a caret stop)
    /// of the visible character at UTF-16 `index`, or the document's end stop at
    /// or past the end of the text.
    pub fn offset_for_utf16_index(&mut self, index: usize) -> usize {
        self.sync();
        let len = self.doc.source.len();
        let end = self.snap_stop(len);
        match self.doc.view {
            View::Wysiwyg => self
                .doc
                .vmap
                .offset_at_visible_utf16(end, index)
                .map_or(end, |o| self.snap_stop(o)),
            View::Source => {
                let mut seen = 0usize;
                for (i, ch) in self.doc.source.char_indices() {
                    let n = ch.len_utf16();
                    if index < seen + n {
                        return i;
                    }
                    seen += n;
                }
                end
            }
        }
    }

    /// The visible text between two offsets. In the WYSIWYG view this is *not*
    /// the raw source slice: a hidden delimiter (`**`, `` ` ``, `_`) contributes
    /// nothing, matching what the reader sees and what a copy takes.
    pub fn text_in_range(&mut self, from: usize, to: usize) -> String {
        self.sync();
        let len = self.doc.source.len();
        let (mut a, mut b) = (from.min(len), to.min(len));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        match self.doc.view {
            View::Wysiwyg => self.doc.vmap.visible_text(a, b),
            View::Source => {
                let s = &self.doc.source;
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

    /// Put the selection at a source range, without a click to place it.
    pub fn set_selection_offsets(
        &mut self,
        anchor: usize,
        focus: usize,
    ) -> Result<DocView, JsValue> {
        self.doc.place_caret(anchor, false);
        if focus != anchor {
            self.doc.place_caret(focus, true);
        }
        self.frame()
    }

    /// Select the exact source range `[start, end)`, snapping neither end to a
    /// visible caret stop — for a host painting a range it already knows the
    /// bytes of (a search hit, an annotation) rather than hit-testing a click.
    ///
    /// `set_selection_offsets` above is the *other* verb: it goes through
    /// `place_caret`, which snaps, and is what a drag handle wants. This one
    /// takes the range as given, so a selection over `**needle**`'s inner word
    /// is the word and not one byte short of it.
    pub fn select_range(&mut self, start: usize, end: usize) -> Result<DocView, JsValue> {
        self.doc.select_range(start, end);
        self.frame()
    }

    /// Replace the source range `[from, to)` with `text`.
    pub fn replace_range(
        &mut self,
        from: usize,
        to: usize,
        text: &str,
    ) -> Result<DocView, JsValue> {
        self.doc.place_caret(from, false);
        if to != from {
            self.doc.place_caret(to, true);
        }
        self.doc.insert(text);
        self.frame()
    }

    /// Stop wrapping: lay every logical line out unbroken, for a surface that
    /// scrolls horizontally instead of folding.
    pub fn set_unwrapped(&mut self) -> Result<DocView, JsValue> {
        // Core takes a column budget, not an option, so "unwrapped" is a budget
        // no line can reach.
        self.width = usize::MAX;
        self.frame()
    }

    // ── links and footnotes ─────────────────────────────────────────────────

    /// The destination of the link at `off`, or `undefined` — for making a
    /// rendered link followable from a click, which knows an offset (a run's
    /// `src`) and not a caret.
    ///
    /// Only a *parsed* link answers: a bare wikilink is literal text with no
    /// node behind it and nothing to point at.
    pub fn link_destination_at(&mut self, off: usize) -> Option<String> {
        self.doc.link_destination_at(off)
    }

    /// The destination of the link the caret stands in, or `undefined`. Also on
    /// every frame as [`DocView::link`]; this is the one-off query.
    pub fn link_destination_at_caret(&mut self) -> Option<String> {
        self.doc.link_destination_at_caret()
    }

    /// The source of the image the caret stands in, or `undefined` — the `src`
    /// of an `![](cat.png)`, as written. What an image prompt seeds from, and
    /// what a host that gives an attachment a page of its own asks before
    /// offering to go there.
    pub fn image_destination_at_caret(&mut self) -> Option<String> {
        self.doc.image_destination_at_caret()
    }

    /// The heading `off` is under — the nearest at or above it — or
    /// `undefined` above the first. What a host writing a link *to* a place
    /// names it by.
    pub fn heading_at(&mut self, off: usize) -> Option<HeadingView> {
        self.doc.heading_at(off).map(HeadingView::from)
    }

    /// The heading the caret is under, or `undefined`.
    pub fn heading_at_caret(&mut self) -> Option<HeadingView> {
        self.doc.heading_at_caret().map(HeadingView::from)
    }

    /// Where a locator lands — the span of the block a fragment id names, for
    /// following an in-document link.
    pub fn locate(&mut self, id: &str) -> Option<LandingView> {
        self.doc.locate(id).map(LandingView::from)
    }

    /// Move the caret's block one place up — Alt+↑: above the block before
    /// it, and out of its container to just above it when it is the first
    /// block there. A list item goes with its children; the caret rides the
    /// block. Gate on `capabilities().move_block`.
    pub fn move_block_up(&mut self) -> Result<DocView, JsValue> {
        self.doc.move_block_up();
        self.frame()
    }

    /// Move the caret's block one place down — the mirror of `move_block_up`.
    pub fn move_block_down(&mut self) -> Result<DocView, JsValue> {
        self.doc.move_block_down();
        self.frame()
    }

    /// Move the block at source offset `from` to the boundary `to` — the drop
    /// half of a drag, `to` from `drop_target_at` and `from` any offset in the
    /// block carried. One undo step; the caret rides the block; a drop back
    /// onto the block's own boundary is a quiet no-op.
    pub fn move_block(&mut self, from: usize, to: usize) -> Result<DocView, JsValue> {
        self.doc.move_block(from, to);
        self.frame()
    }

    /// The source range of the block a drag starting at `(row, ch)` picks up
    /// — the whole paragraph, picture, table or fence, or the whole list
    /// item with its children — for the outline drawn under the pointer.
    /// `undefined` on a blank line. The caret is untouched; map the pair
    /// through `row_range_for` for its rows.
    pub fn block_range_at(&mut self, row: usize, ch: usize) -> Option<LandingView> {
        let off = self.offset_for_pos(row, ch);
        self.doc.block_range_at(off).map(|r| LandingView {
            start: r.start,
            end: r.end,
        })
    }

    /// Where a block dragged over rendered `row` would land: the boundary
    /// before the row's block when the row is in its upper half, after it
    /// otherwise, the document's end for a row below everything. `undefined`
    /// for a row with no block under it.
    pub fn drop_target_at(&mut self, row: usize) -> Option<DropTargetView> {
        self.sync();
        self.doc.drop_target_at(row).map(DropTargetView::from)
    }

    /// Write a footnote reference at the caret and the definition it needs.
    pub fn insert_footnote(&mut self) -> Result<DocView, JsValue> {
        self.doc.insert_footnote();
        self.frame()
    }

    /// The footnote reference at `off` and the note it names, or `undefined` if
    /// there is no reference there.
    ///
    /// A reference whose definition the document is missing still answers, with
    /// its label and no text: that a `[^99]` names nothing is worth telling the
    /// reader, and is not the same as standing on no reference at all.
    pub fn footnote_at(&mut self, off: usize) -> Option<FootnoteView> {
        self.doc.footnote_at(off).map(FootnoteView::from)
    }

    /// The footnote reference the caret stands on, or `undefined`.
    pub fn footnote_at_caret(&mut self) -> Option<FootnoteView> {
        self.doc.footnote_at_caret().map(FootnoteView::from)
    }

    /// The footnote *definition* the caret stands in, and the first reference
    /// that sends a reader to it — the return leg of the round trip.
    pub fn footnote_definition_at_caret(&mut self) -> Option<FootnoteDefView> {
        self.doc
            .footnote_definition_at_caret()
            .map(FootnoteDefView::from)
    }

    /// Write a thematic break (`---`) at the caret.
    pub fn insert_thematic_break(&mut self) -> Result<DocView, JsValue> {
        self.doc.insert_thematic_break();
        self.frame()
    }

    // ── tables ──────────────────────────────────────────────────────────────
    //
    // Gate these on `caret_in_table` *and* on `capabilities().table`: the first
    // asks whether the caret is in a grid, the second whether this format's
    // tables are editable at all. An HTML `<table>` answers yes to the first and
    // no to the second.

    /// Whether the caret is inside a table.
    pub fn caret_in_table(&mut self) -> bool {
        self.doc.caret_in_table()
    }

    pub fn table_insert_row(&mut self, below: bool) -> Result<DocView, JsValue> {
        self.doc.table_insert_row(below);
        self.frame()
    }

    pub fn table_delete_row(&mut self) -> Result<DocView, JsValue> {
        self.doc.table_delete_row();
        self.frame()
    }

    pub fn table_insert_column(&mut self, right: bool) -> Result<DocView, JsValue> {
        self.doc.table_insert_column(right);
        self.frame()
    }

    pub fn table_delete_column(&mut self) -> Result<DocView, JsValue> {
        self.doc.table_delete_column();
        self.frame()
    }

    pub fn table_move_row(&mut self, down: bool) -> Result<DocView, JsValue> {
        self.doc.table_move_row(down);
        self.frame()
    }

    pub fn table_move_column(&mut self, right: bool) -> Result<DocView, JsValue> {
        self.doc.table_move_column(right);
        self.frame()
    }

    /// Insert a fresh table at the caret — one header row, `rows` empty body
    /// rows, `cols` columns — and leave the caret in its first header cell.
    /// Needs no table under the caret; gate it on `capabilities().table`.
    pub fn insert_table(&mut self, rows: u32, cols: u32) -> Result<DocView, JsValue> {
        self.doc.insert_table(rows as usize, cols as usize);
        self.frame()
    }

    /// Set the caret's column alignment — `"left"`, `"right"`, `"center"`, or
    /// `"default"`. Anything else is left alone.
    pub fn table_set_alignment(&mut self, alignment: &str) -> Result<DocView, JsValue> {
        let a = match alignment.to_ascii_lowercase().as_str() {
            "left" => Alignment::Left,
            "right" => Alignment::Right,
            "center" => Alignment::Center,
            "default" => Alignment::Default,
            other => return Err(JsValue::from_str(&format!("unknown alignment: {other}"))),
        };
        self.doc.table_set_alignment(a);
        self.frame()
    }

    /// Tab inside a table: to the next (or previous) cell, adding a row when it
    /// steps off the end. `undefined` when the caret isn't in one, so the host
    /// can fall through to its ordinary Tab (indent).
    pub fn cell_tab(&mut self, forward: bool) -> Result<Option<DocView>, JsValue> {
        if self.doc.cell_tab(forward) {
            self.frame().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Return inside a table: to the cell below, adding a row at the last one.
    /// `undefined` when the caret isn't in a table.
    pub fn cell_return(&mut self) -> Result<Option<DocView>, JsValue> {
        if self.doc.cell_return() {
            self.frame().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Shift+Return inside a cell: a line break *within* the cell rather than a
    /// new row. `undefined` when the caret isn't in a table.
    pub fn cell_line_break(&mut self) -> Result<Option<DocView>, JsValue> {
        if self.doc.cell_line_break() {
            self.frame().map(Some)
        } else {
            Ok(None)
        }
    }
}

/// The WYSIWYG rows: each visual row's glyphs coalesced into maximal runs of
/// identical `(style, selected)` — the same span merge the TUI does. A glyph is
/// selected when its source byte lies in `[ss, se)`.
/// Every formula in `vmap` as the renderer's [`MathView`]s — the peer of
/// [`media_views`].
fn math_views(vmap: &VisualMap) -> Vec<MathView> {
    vmap.math
        .iter()
        .map(|m| MathView {
            row: m.rows_span.start,
            rows: m.rows_span.len().max(1),
            inline: m.glyph.is_some(),
            tex: m.tex.clone(),
            display: m.display,
            src: m.src,
        })
        .collect()
}

/// Every block media in `vmap` as the renderer's [`MediaView`]s, with each URL
/// already resolved under `scheme`.
///
/// Resolving here rather than in JS keeps the one piece of `<picture>` logic
/// core owns (`prefers-color-scheme` matching) in core, and hands the renderer a
/// URL it can use directly. The `<source>` list still goes across untouched, so
/// the browser can *also* do its own native picking on a `<video>`'s codecs —
/// something core has no business judging.
fn media_views(vmap: &VisualMap, scheme: ColorScheme) -> Vec<MediaView> {
    vmap.media
        .iter()
        .map(|m| MediaView {
            row: m.rows_span.start,
            rows: m.rows_span.len().max(1),
            kind: match m.kind {
                MediaKind::Image => "image",
                MediaKind::Video => "video",
                MediaKind::Audio => "audio",
            }
            .to_string(),
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

fn wysiwyg_rows(vmap: &VisualMap, ss: usize, se: usize, hls: &[CoreHighlight]) -> Vec<Row> {
    // The map's own face table, for the family name a glyph carries only an id
    // for — threaded down beside `hls`, which travels the same road.
    let faces = vmap.faces();
    vmap.rows
        .iter()
        .map(|vrow| {
            // The row's heading level, if any — straight off the row rather than
            // scanned out of its glyphs (see [`Row::heading`]). An empty heading
            // (`# ` with nothing typed yet) has no glyph to read a role from, and
            // a renderer sizing the line by one drew it at body height until the
            // first character landed.
            let heading = vrow.heading;

            Row {
                runs: runs_of(&vrow.glyphs, ss, se, hls, faces),
                decoration: vrow.decoration,
                code: vrow.code,
                code_lang: vrow.code_lang.clone(),
                directive: vrow.directive,
                directive_label: vrow.directive_label.clone(),
                boundary: vrow.boundary.map(|b| BoundaryView {
                    above: class_name(b.above),
                    below: class_name(b.below),
                }),
                heading,
                // Off the row for `heading`'s reason, and more sharply: these
                // are properties of the *line*, so an empty paragraph just
                // centred has no run to carry them.
                align: vrow.align.map(|a| a.name().to_string()),
                line_height: vrow.line_height.map(|l| l.name().to_string()),
            }
        })
        .collect()
}

/// The source rows: the raw document split on `'\n'`, each line cut into runs
/// wherever its styling changes — the markup `smap` colours, the `[ss, se)`
/// selection, and the host's highlights — the browser counterpart of the TUI's
/// `build_lines`. This is what backs the source view, whose caret rides raw
/// byte offsets (see `Doc::caret_pos`).
///
/// An empty `smap` — a frontend that never built one, a document with no
/// markup — paints every line as plain text, which is what this did before
/// the map reached it.
fn source_rows(
    source: &str,
    smap: &SourceMap,
    ss: usize,
    se: usize,
    hls: &[CoreHighlight],
) -> Vec<Row> {
    // Raw text carries no attributed span, so no run of it names a family and
    // the table it would be read out of is empty.
    let faces = CoreFaceTable::default();
    let mut rows = Vec::new();
    let mut byte = 0usize;
    // Reused across lines rather than allocated per line: a document is a few
    // thousand of them and this is rebuilt on every whole frame.
    let mut cuts: Vec<usize> = Vec::new();
    // Which highlight covers a byte — first by start when several overlap,
    // matching `Doc::highlight_at` and `runs_of` above.
    let hl_of = |src: usize| hls.iter().position(|h| h.start <= src && src < h.end);

    for raw in source.split('\n') {
        let start = byte;
        let end = start + raw.len();

        // Where the styling can change within this line, in document offsets:
        // its two ends, every selection and highlight edge inside it, and every
        // edge of the syntax map's runs. No style edge falls strictly inside a
        // run by construction, so one probe at each run's first byte answers
        // for all of it.
        cuts.clear();
        cuts.push(start);
        cuts.push(end);
        for at in [ss, se]
            .into_iter()
            .chain(hls.iter().flat_map(|h| [h.start, h.end]))
        {
            if at > start && at < end {
                cuts.push(at);
            }
        }
        smap.edges_in(start..end, &mut cuts);
        cuts.sort_unstable();
        cuts.dedup();

        let mut runs = Vec::new();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            runs.push(make_run(
                raw[a - start..b - start].to_string(),
                smap.style_at(a),
                a >= ss && a < se,
                hl_of(a).map(|i| &hls[i]),
                a,
                &faces,
            ));
        }

        rows.push(Row {
            runs,
            decoration: false,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            boundary: None,
            heading: None, // source view is raw text — no resolved structure
            align: None,   // …and so no attributes resolved onto a block
            line_height: None,
        });
        byte = end + 1; // skip the '\n' that `split` consumed
    }
    rows
}

/// Build a [`Run`] from an accumulated string and the core style it was drawn
/// with — the one place role, emphasis, and baseline cross into the view shape.
fn make_run(
    text: String,
    style: LStyle,
    sel: bool,
    hl: Option<&CoreHighlight>,
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
        src,
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

/// A presentation value by the token a document spells it with — the argument
/// each of the six gestures below takes, and the one shape all six have.
///
/// `parse` says what the token means: `Some(Some(value))` for a value,
/// `Some(None)` for a token *in* the grammar that means the theme's own (only
/// a line spacing has one — `"1"` is single, which has no token), and `None`
/// for a token outside the grammar.
///
/// `None` in is the theme's own too, and both clearings answer `Ok(None)`. A
/// token outside the grammar is an **error** rather than a silent clearing:
/// the argument that clears is `null`, and the two differ by one typo and mean
/// opposite things.
fn presentation<T>(
    token: Option<&str>,
    what: &str,
    parse: impl FnOnce(&str) -> Option<Option<T>>,
) -> Result<Option<T>, JsValue> {
    match token {
        None => Ok(None),
        Some(token) => {
            parse(token).ok_or_else(|| JsValue::from_str(&format!("unknown {what}: {token}")))
        }
    }
}

/// A highlight colour by the name a document records it under — the argument
/// `setMarkColor` and `highlight` take. `None` is no colour; a name outside the
/// palette is an error, for [`presentation`]'s reason.
fn mark_color(name: Option<&str>) -> Result<Option<CoreMarkColor>, JsValue> {
    presentation(name, "highlight colour", |n| {
        CoreMarkColor::from_attr(n).map(Some)
    })
}

/// A block alignment by the `class` token a document spells it with — the
/// argument `set_alignment` takes. `None` is the theme's default (there is no
/// `left` token, because absence is left); anything else outside the vocabulary
/// is an error.
fn alignment(token: Option<&str>) -> Result<Option<CoreAlign>, JsValue> {
    presentation(token, "alignment", |t| CoreAlign::from_token(t).map(Some))
}

/// A line spacing by the token a document spells it with — the argument
/// `set_line_spacing` takes: one of the three names, or any other positive
/// decimal. `None` is the theme's spacing, and so is `"1"` — single spacing,
/// which is the theme's own and has no token of its own.
///
/// The one helper here whose grammar holds a value meaning *absence*, which is
/// why it asks [`CoreLineHeight::is_absence`]: `from_attr` answers `None` for
/// `"1"` exactly as it does for `"huge"`, and the two mean opposite things —
/// the first is an author asking for single spacing and clears the key, the
/// second is a typo and is refused.
fn line_spacing(name: Option<&str>) -> Result<Option<CoreLineHeight>, JsValue> {
    presentation(name, "line spacing", |n| {
        match CoreLineHeight::from_attr(n) {
            Some(height) => Some(Some(height)),
            None => CoreLineHeight::is_absence(n).then_some(None),
        }
    })
}

/// A size by the token a document spells it with — the argument `set_font_size`
/// takes: one of CSS's seven keywords, or a `<number>pt`. `None` is the theme's
/// own size, which is what `"medium"` would mean and is why it is not a value.
fn font_size(name: Option<&str>) -> Result<Option<CoreFontSize>, JsValue> {
    presentation(name, "font size", |n| CoreFontSize::from_attr(n).map(Some))
}

/// A face by the token a document spells it with — the argument
/// `set_font_family` takes: one of CSS's four generics, or a family name.
/// `None` is the theme's body face; a name that spells a generic is read as
/// that generic, and a name that is only whitespace names nothing and is an
/// error.
fn font_family(name: Option<&str>) -> Result<Option<CoreFontFace>, JsValue> {
    presentation(name, "font family", |n| {
        CoreFontFace::from_attr(n).map(Some)
    })
}

/// A *text* colour by the token a document spells it with — the argument
/// `set_text_color` takes: one of the seven names, `#rrggbb`, or the `#rgb`
/// shorthand. `None` is the theme's ink; anything else is an error.
fn text_color(name: Option<&str>) -> Result<Option<CoreTextColor>, JsValue> {
    presentation(name, "text colour", |n| {
        CoreTextColor::from_attr(n).map(Some)
    })
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

/// Coalesce `glyphs` into maximal runs of identical `(style, selected)` — the
/// shared body of a row's runs and a table cell's. A glyph is selected when its
/// source byte lies in `[ss, se)`, so the selection splits a run exactly as a
/// style change does.
fn runs_of(
    glyphs: &[Glyph],
    ss: usize,
    se: usize,
    hls: &[CoreHighlight],
    faces: &CoreFaceTable,
) -> Vec<Run> {
    // Which highlight (by index) covers a glyph — first by start when several
    // overlap, matching `Doc::highlight_at`. Part of the run key: a highlight
    // splits a run exactly the way the selection does.
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

/// Split a cell's flat glyphs into its visual lines at the in-cell break glyphs
/// (`\n`, from a `<br>`), each with the source range it spans. A line runs from
/// its first glyph's offset to the break that ends it (`cell_end` for the last);
/// an empty line — a leading/trailing break, or an empty cell — collapses to a
/// single caret home. The break glyphs themselves are dropped (they hold no
/// caret), exactly as the monospace picture drops them.
fn cell_lines(
    glyphs: &[Glyph],
    cell_start: usize,
    cell_end: usize,
    ss: usize,
    se: usize,
    hls: &[CoreHighlight],
    faces: &CoreFaceTable,
) -> Vec<TableCellLineView> {
    let mut lines = Vec::new();
    let mut seg: Vec<Glyph> = Vec::new();
    // The current line's start offset: the cell's for the first line, then the
    // first real glyph after each break (`None` until that glyph is seen).
    let mut line_start: Option<usize> = Some(cell_start);
    for g in glyphs {
        if g.ch == '\n' {
            let start = line_start.unwrap_or(g.src);
            lines.push(TableCellLineView {
                runs: runs_of(&seg, ss, se, hls, faces),
                start,
                end: g.src,
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
        start: line_start.unwrap_or(cell_end),
        end: cell_end,
    });
    lines
}

/// The structural tables of a WYSIWYG frame — each with the `rows` span its
/// box-glyph picture occupies (to be skipped) and its grid of styled cells.
fn wysiwyg_tables(vmap: &VisualMap, ss: usize, se: usize, hls: &[CoreHighlight]) -> Vec<TableView> {
    let faces = vmap.faces();
    vmap.tables
        .iter()
        .map(|t| TableView {
            start_row: t.rows_span.start,
            end_row: t.rows_span.end,
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
                            start: cell.start,
                            end: cell.end,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

/// The leaf directives of a WYSIWYG frame, each naming the placeholder rows a
/// host-aware renderer replaces.
fn wysiwyg_directives(vmap: &VisualMap) -> Vec<DirectiveView> {
    vmap.directives
        .iter()
        .map(|d| DirectiveView {
            start_row: d.rows_span.start,
            end_row: d.rows_span.end,
            name: d.name.clone(),
            label: d.label.clone(),
            attrs: d
                .attrs
                .iter()
                .map(|(key, value)| DirectiveAttr {
                    key: key.clone(),
                    // A bare attribute is a flag; the difference from `key=""`
                    // has no consumer on this side.
                    value: value.clone().unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a WYSIWYG map the way [`LeafDoc::view`] does, without needing a
    /// `JsValue` — the view-producing methods return one, so they can't be
    /// called off wasm, but everything they assemble the frame *from* is
    /// ordinary Rust and is what these tests exercise.
    fn wysiwyg(source: &str) -> Doc {
        let mut doc = Doc::from_source(source.to_string(), Format::Markdown).unwrap();
        doc.build_visual(80);
        doc
    }

    /// The source view's rows carry the markup's styling and split at its
    /// edges, the selection's and a highlight's — the same runs the FFI hands
    /// Swift and the TUI paints, from the same map.
    #[test]
    fn source_rows_carry_the_source_maps_styling() {
        let mut doc = Doc::from_source("a **bold** b\n".to_string(), Format::Markdown).unwrap();
        doc.build_source();
        let src = doc.source.clone();
        let b = src.find("bold").unwrap();
        let hls = [CoreHighlight {
            start: 0,
            end: 3, // "a *"
            id: "h".into(),
            color: None,
            marker: None,
        }];
        // The selection is "ol", inside the bold run.
        let rows = source_rows(&src, &doc.smap, b + 1, b + 3, &hls);
        let runs: Vec<(&str, &str, bool, bool, Option<&str>)> = rows[0]
            .runs
            .iter()
            .map(|r| {
                (
                    r.text.as_str(),
                    r.role.as_str(),
                    r.bold,
                    r.sel,
                    r.hl.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            runs,
            vec![
                ("a ", "body", false, false, Some("h")),
                ("*", "delimiter", true, false, Some("h")),
                ("*", "delimiter", true, false, None),
                ("b", "body", true, false, None),
                ("ol", "body", true, true, None),
                ("d", "body", true, false, None),
                ("**", "delimiter", true, false, None),
                (" b", "body", false, false, None),
            ]
        );
        // Every run knows where it came from, as the rendered rows' do.
        assert_eq!(rows[0].runs[4].src, b + 1);
        // And an empty map paints plain text, as this always did.
        let plain = source_rows(&src, &SourceMap::default(), 0, 0, &[]);
        assert_eq!(plain[0].runs.len(), 1);
        assert_eq!(plain[0].runs[0].role, "body");
    }

    /// A fenced block's body carries the grammar's tokens in the source view
    /// as it does in the rendered one.
    #[cfg(feature = "syntax")]
    #[test]
    fn source_rows_carry_fence_tokens() {
        let mut doc =
            Doc::from_source("```rust\nlet x = 1;\n```\n".to_string(), Format::Markdown).unwrap();
        doc.build_source();
        let rows = source_rows(&doc.source, &doc.smap, 0, 0, &[]);
        let kw = rows[1]
            .runs
            .iter()
            .find(|r| r.text == "let")
            .expect("a run for the keyword");
        assert_eq!(kw.role, "code");
        assert_eq!(kw.token.as_deref(), Some("keyword"));
        assert_eq!(rows[0].runs[0].role, "delimiter", "the fence is markup");
    }

    #[test]
    fn a_column_and_a_utf16_offset_agree_only_on_ascii() {
        // Two cells wide, one UTF-16 unit: the two measures diverge immediately.
        assert_eq!(col_to_utf16("漢字", 2), 1);
        assert_eq!(utf16_to_col("漢字", 1), 2);
        // An astral emoji is two cells *and* two UTF-16 units, for different reasons.
        assert_eq!(col_to_utf16("🍃x", 2), 2);
        assert_eq!(utf16_to_col("🍃x", 2), 2);
        // Plain ASCII is the one case where they coincide.
        assert_eq!(col_to_utf16("leaf", 3), 3);
        assert_eq!(utf16_to_col("leaf", 3), 3);
    }

    /// A column falling inside a wide cluster resolves to the boundary *after*
    /// it — clusters are consumed whole. Core never asks for such a column (a
    /// caret column is a cluster start); this pins down what happens if anything
    /// ever does, so the answer is a boundary rather than a split cluster.
    #[test]
    fn a_column_inside_a_wide_cluster_resolves_past_it() {
        assert_eq!(col_to_utf16("漢字", 1), 1);
        assert_eq!(col_to_utf16("漢字", 3), 2);
    }

    /// A token splits a run the way a style does — it *is* part of the style
    /// — and rides across as its class id. A block in a language no grammar
    /// covers is one plain `code` run with no token, as it always was.
    #[cfg(feature = "syntax")]
    #[test]
    fn a_highlighted_block_splits_its_runs_by_token() {
        let doc = wysiwyg("```rust\nlet x = 1;\n```\n");
        let row = doc.vmap.rows.iter().find(|r| r.code).expect("a code row");
        let runs = runs_of(
            &row.glyphs,
            usize::MAX,
            usize::MAX,
            &[],
            &CoreFaceTable::default(),
        );
        let classed: Vec<(&str, Option<&str>)> = runs
            .iter()
            .map(|r| (r.text.as_str(), r.token.as_deref()))
            .collect();
        assert_eq!(classed[0], ("let", Some("keyword")));
        assert!(runs.iter().all(|r| r.role == "code"), "{classed:?}");
        assert!(
            classed
                .iter()
                .any(|(t, k)| *t == "1" && *k == Some("constant")),
            "{classed:?}"
        );

        let plain = wysiwyg("```text\nlet x = 1;\n```\n");
        let row = plain.vmap.rows.iter().find(|r| r.code).unwrap();
        let runs = runs_of(
            &row.glyphs,
            usize::MAX,
            usize::MAX,
            &[],
            &CoreFaceTable::default(),
        );
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].token, None);
    }

    #[test]
    fn runs_coalesce_by_style_and_split_on_the_selection_edge() {
        let doc = wysiwyg("plain **bold** plain\n");
        let glyphs = &doc.vmap.rows[0].glyphs;

        let runs = runs_of(
            glyphs,
            usize::MAX,
            usize::MAX,
            &[],
            &CoreFaceTable::default(),
        );
        let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["plain ", "bold", " plain"]);
        assert!(runs.iter().all(|r| !r.sel));
        assert!(runs[1].bold && !runs[0].bold);

        // Each run's `src` is its first glyph's offset, so it points back into
        // the source rather than into the rendered text.
        assert_eq!(runs[0].src, 0);
        assert_eq!(&doc.source[runs[1].src..runs[1].src + 4], "bold");

        // A selection edge inside a styled span splits that span in two, and the
        // two halves keep the style.
        let start = doc.source.find("bold").unwrap();
        let split = runs_of(glyphs, start, start + 2, &[], &CoreFaceTable::default());
        let selected: Vec<&str> = split
            .iter()
            .filter(|r| r.sel)
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(selected, ["bo"]);
        assert!(split.iter().filter(|r| r.bold).count() == 2);
    }

    /// A footnote reference is drawn raised; the flag has to reach the renderer,
    /// because CSS is the only thing that can make it look raised.
    #[test]
    fn a_raised_run_says_so() {
        let doc = wysiwyg("text[^1]\n\n[^1]: note\n");
        let sup: Vec<String> = doc
            .vmap
            .rows
            .iter()
            .flat_map(|r| {
                runs_of(
                    &r.glyphs,
                    usize::MAX,
                    usize::MAX,
                    &[],
                    &CoreFaceTable::default(),
                )
            })
            .filter(|r| r.sup)
            .map(|r| r.text.clone())
            .collect();
        assert!(!sup.is_empty(), "no run came across raised: {sup:?}");
        assert!(sup.iter().all(|t| !t.is_empty()));
        assert!(
            doc.vmap
                .rows
                .iter()
                .flat_map(|r| runs_of(
                    &r.glyphs,
                    usize::MAX,
                    usize::MAX,
                    &[],
                    &CoreFaceTable::default()
                ))
                .all(|r| !(r.sup && r.sub)),
            "a run cannot be both raised and lowered"
        );
    }

    /// The colour rides beside the role rather than inside it, so the
    /// stylesheet's `.leaf-r-mark` rule still catches a coloured highlight and
    /// `.leaf-mk-red` only swaps the wash on top of it.
    #[test]
    fn a_coloured_highlight_crosses_as_a_name_beside_the_mark_role() {
        let doc = wysiwyg("a ==\u{1F534} red== and ==plain== b\n");
        let marks: Vec<(String, Option<String>)> = doc
            .vmap
            .rows
            .iter()
            .flat_map(|r| {
                runs_of(
                    &r.glyphs,
                    usize::MAX,
                    usize::MAX,
                    &[],
                    &CoreFaceTable::default(),
                )
            })
            .filter(|r| r.role == "mark")
            .map(|r| (r.text.clone(), r.mark_color.clone()))
            .collect();
        assert_eq!(
            marks,
            [
                ("red".to_string(), Some("red".to_string())),
                ("plain".to_string(), None),
            ]
        );
    }

    #[test]
    fn a_table_crosses_as_a_grid_and_names_the_picture_rows_to_skip() {
        let doc = wysiwyg("| a | b |\n|---|--:|\n| 1 | 2 |\n");
        let tables = wysiwyg_tables(&doc.vmap, usize::MAX, usize::MAX, &[]);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];

        // The header row plus one body row — the `|---|` is an alignment spec,
        // not a row of content.
        assert_eq!(t.grid.len(), 2);
        assert!(t.grid[0].head);
        assert!(!t.grid[1].head);
        assert_eq!(t.grid[0].cells.len(), 2);

        let text = |c: &TableCellView| -> String {
            c.lines
                .iter()
                .flat_map(|l| l.runs.iter())
                .map(|r| r.text.as_str())
                .collect::<String>()
                .trim()
                .to_string()
        };
        assert_eq!(text(&t.grid[0].cells[0]), "a");
        assert_eq!(text(&t.grid[1].cells[1]), "2");

        // `|--:|` is a right-aligned column, and the alignment rides the cell.
        assert_eq!(t.grid[0].cells[1].align, "right");
        assert_eq!(t.grid[0].cells[0].align, "default");

        // The rows the box-glyph picture occupies really are the drawn ones, and
        // a renderer skipping them skips the whole table.
        assert!(t.end_row > t.start_row);
        assert!(t.end_row <= doc.vmap.rows.len());

        // A cell's source range addresses its own text, which is what makes a
        // click in a drawn cell land on the right caret offset.
        let cell = &t.grid[1].cells[0];
        assert!(doc.source[cell.start..cell.end].contains('1'));
    }

    /// The two descriptions of a table are alternatives, not layers: whatever a
    /// renderer skips in `rows`, it must find in the grid.
    #[test]
    fn every_table_cell_sits_inside_the_source() {
        let doc = wysiwyg("| one | two |\n|---|---|\n| three | four |\n");
        for t in wysiwyg_tables(&doc.vmap, usize::MAX, usize::MAX, &[]) {
            for row in &t.grid {
                for cell in &row.cells {
                    assert!(cell.start <= cell.end);
                    assert!(cell.end <= doc.source.len());
                    for line in &cell.lines {
                        assert!(line.start <= line.end, "{}..{}", line.start, line.end);
                        assert!(line.end <= doc.source.len());
                    }
                }
            }
        }
    }

    /// A live handle, at a width wide enough that nothing wraps.
    ///
    /// Off wasm, `JsValue` is a stub that panics when touched — so a method
    /// *returning* one can't be called here, but the offset-addressed methods
    /// return plain values and can. Those are exactly the ones a host uses to
    /// point at part of the document, so they are the ones worth pinning down.
    fn handle(source: &str) -> LeafDoc {
        let mut d = LeafDoc::new(source, "markdown").expect("markdown parses");
        d.width = 200;
        d.sync();
        d
    }

    #[test]
    fn an_offset_and_a_position_round_trip() {
        let mut d = handle("# heading\n\nsome **bold** words\n");
        let off = d.doc.source.find("bold").unwrap();
        let pos = d.pos_for_offset(off);
        // Back the other way lands on the same byte — the two are inverses over
        // the offsets core actually publishes a caret stop for.
        assert_eq!(d.offset_for_pos(pos.row, pos.ch), off);
    }

    #[test]
    fn stepping_and_measuring_agree_about_the_distance_between_two_stops() {
        let mut d = handle("abcdef\n");
        let start = d.snap_offset(0);
        let three = d.step_offset(start, 3);
        assert_eq!(d.distance_offset(start, three), 3);
        // Signed by direction, so the reverse is the negative.
        assert_eq!(d.distance_offset(three, start), -3);
        // Stepping past the end stops there rather than running away.
        let end = d.doc_end_offset();
        assert_eq!(d.step_offset(end, 50), end);
        assert_eq!(d.step_offset(start, -50), d.step_offset(start, -1));
    }

    /// The WYSIWYG view's text is what the reader sees, so a hidden delimiter
    /// contributes nothing — which is also what a copy out of the surface takes.
    #[test]
    fn visible_text_leaves_the_hidden_delimiters_out() {
        let mut d = handle("a **bold** b\n");
        let all = d.text_in_range(0, d.doc.source.len());
        assert!(all.contains("bold"));
        assert!(
            !all.contains("**"),
            "delimiters leaked into visible text: {all:?}"
        );
        // Reversed bounds describe the same range.
        assert_eq!(d.text_in_range(4, 2), d.text_in_range(2, 4));
    }

    #[test]
    fn a_footnote_reference_finds_its_note_and_the_note_finds_it_back() {
        let mut d = handle("cited[^1] here\n\n[^1]: the note itself\n");
        let marker = d.doc.source.find("[^1]").unwrap();
        let note = d.footnote_at(marker).expect("a reference sits there");
        assert_eq!(note.label, "1");
        assert!(
            note.text
                .as_deref()
                .unwrap_or("")
                .contains("the note itself")
        );

        // The pair bounds the note, so a renderer can ask which rows to draw.
        let (start, end) = (note.offset.unwrap(), note.end.unwrap());
        assert!(start < end && end <= d.doc.source.len());
        let rows = d.row_range_for(start, end);
        assert!(rows.last >= rows.first);
    }

    /// A reference nothing defines still answers — "this note is missing" is
    /// worth saying, and is not the same as standing on no reference at all.
    #[test]
    fn a_reference_with_no_definition_still_answers() {
        let mut d = handle("dangling[^99] here\n");
        // Core may decline to parse a reference nothing defines; what must not
        // happen is one reported *with* a note it hasn't got.
        if let Some(n) = d.footnote_at(d.doc.source.find("[^99]").unwrap()) {
            assert_eq!(n.label, "99");
            assert!(n.text.is_none() && n.offset.is_none());
        }
        assert!(d.footnote_at(0).is_none(), "no reference at the line start");
    }

    #[test]
    fn a_heading_answers_for_the_text_under_it() {
        let mut d = handle("intro\n\n## The *Second* Part\n\nbody\n");
        assert!(d.heading_at(0).is_none());
        let h = d.heading_at(d.doc.source.find("body").unwrap()).unwrap();
        assert_eq!((h.text.as_str(), h.level), ("The Second Part", 2));
        assert_eq!(h.start, d.doc.source.find("##").unwrap());
    }

    #[test]
    fn a_link_answers_at_its_own_offset_and_nowhere_else() {
        let mut d = handle("see [the docs](https://example.com/x) now\n");
        let inside = d.doc.source.find("the docs").unwrap() + 2;
        assert_eq!(
            d.link_destination_at(inside).as_deref(),
            Some("https://example.com/x")
        );
        assert!(d.link_destination_at(0).is_none());
    }

    /// The block half of the presentation vocabulary rides the **row**, as the
    /// token a stylesheet selects on: `.center` is the class and
    /// `[data-line-height="1.5"]` the attribute, so the renderer puts both on
    /// the row element and `presentation.css` does the rest. A run could not
    /// carry them — an empty paragraph the author has just centred hasn't got
    /// one.
    #[test]
    fn the_block_vocabulary_crosses_on_the_row_and_comes_back_at_the_caret() {
        let mut d = handle("a centred paragraph\n");
        assert!(d.set_alignment(Some("center".into())).is_ok());
        let rows = wysiwyg_rows(&d.doc.vmap, usize::MAX, usize::MAX, &[]);
        let aligned: Vec<&str> = rows.iter().filter_map(|r| r.align.as_deref()).collect();
        assert_eq!(aligned, ["center"], "the token, on the paragraph's row");
        assert_eq!(d.alignment_at_caret().as_deref(), Some("center"));

        // A second property on the same block keeps the first: each gesture
        // edits one key and passes the rest back whole. (The caret goes back
        // into the paragraph first — in Markdown the attributes went onto a
        // `div` the press wrote *around* it, so the offset the caret kept is now
        // that opening line, which is no block of the document's.)
        let at = d.doc.source.find("centred").unwrap();
        d.doc.place_caret(at, false);
        assert!(d.set_line_spacing(Some("1.5".into())).is_ok());
        let rows = wysiwyg_rows(&d.doc.vmap, usize::MAX, usize::MAX, &[]);
        let block = rows.iter().find(|r| r.align.is_some()).expect("the block");
        assert_eq!(block.align.as_deref(), Some("center"));
        assert_eq!(block.line_height.as_deref(), Some("1.5"));
        assert_eq!(d.line_spacing_at_caret().as_deref(), Some("1.5"));

        // `null` clears, and absence is the theme's default rather than a token
        // meaning "left".
        let at = d.doc.source.find("centred").unwrap();
        d.doc.place_caret(at, false);
        assert!(d.set_alignment(None).is_ok());
        let rows = wysiwyg_rows(&d.doc.vmap, usize::MAX, usize::MAX, &[]);
        assert!(rows.iter().all(|r| r.align.is_none()));
        assert_eq!(d.alignment_at_caret(), None);
        assert_eq!(
            d.line_spacing_at_caret().as_deref(),
            Some("1.5"),
            "clearing one key leaves the other standing"
        );
    }

    /// The run half rides the **run**, beside `role`: `data-size`, `data-font`
    /// and `data-color` as the document spells them, so the renderer writes the
    /// same three attributes onto the span it draws. With nothing selected the
    /// caret's whole block takes them, which is what makes "make this paragraph
    /// larger" one press rather than a select-all first.
    #[test]
    fn the_run_vocabulary_crosses_on_the_run_and_comes_back_at_the_caret() {
        let mut d = handle("big serif blue\n");
        assert!(d.set_font_size(Some("large".into())).is_ok());
        let at = d.doc.source.find("serif").unwrap();
        d.doc.place_caret(at, false);
        assert!(d.set_font_family(Some("serif".into())).is_ok());
        let at = d.doc.source.find("serif").unwrap();
        d.doc.place_caret(at, false);
        assert!(d.set_text_color(Some("blue".into())).is_ok());

        let name = |s: &str| Some(s.to_string());
        let styled: Vec<(Option<String>, Option<String>, Option<String>)> = d
            .doc
            .vmap
            .rows
            .iter()
            .flat_map(|r| {
                runs_of(
                    &r.glyphs,
                    usize::MAX,
                    usize::MAX,
                    &[],
                    &CoreFaceTable::default(),
                )
            })
            .filter(|r| !r.text.trim().is_empty())
            .map(|r| (r.size, r.font, r.text_color))
            .collect();
        assert_eq!(styled, [(name("large"), name("serif"), name("blue"))]);

        assert_eq!(d.font_size_at_caret().as_deref(), Some("large"));
        assert_eq!(d.font_family_at_caret().as_deref(), Some("serif"));
        assert_eq!(d.text_color_at_caret().as_deref(), Some("blue"));
        // A run's text colour is not a highlight's wash — nothing here is a
        // `mark`, and the palette that colours one reports nothing.
        assert!(!d.caret_in_mark());
    }

    /// The other half of each open vocabulary: the value an *Other…* field
    /// writes, out through the gesture and back through both the query and the
    /// run view — in its one canonical spelling, since a stylesheet keys on the
    /// token and a renderer that cannot enumerate it sets it inline.
    ///
    /// The named face is the one form a glyph cannot carry as itself, so this
    /// reads the runs through the map's own table rather than an empty one.
    #[test]
    fn an_exact_value_crosses_as_its_own_token_and_comes_back_whole() {
        let mut d = handle("exact\n");
        let in_text = |d: &mut LeafDoc| {
            let at = d.doc.source.find("exact").unwrap();
            d.doc.place_caret(at, false);
        };
        assert!(d.set_font_size(Some("14pt".into())).is_ok());
        in_text(&mut d);
        assert!(d.set_font_family(Some("Garamond".into())).is_ok());
        in_text(&mut d);
        assert!(d.set_text_color(Some("#c03030".into())).is_ok());
        in_text(&mut d);
        assert!(d.set_line_spacing(Some("1.3".into())).is_ok());

        let rows = wysiwyg_rows(&d.doc.vmap, usize::MAX, usize::MAX, &[]);
        let name = |s: &str| Some(s.to_string());
        let styled: Vec<(Option<String>, Option<String>, Option<String>)> = rows
            .iter()
            .flat_map(|r| r.runs.iter())
            .filter(|r| !r.text.trim().is_empty())
            .map(|r| (r.size.clone(), r.font.clone(), r.text_color.clone()))
            .collect();
        assert_eq!(
            styled,
            [(name("14pt"), name("Garamond"), name("#c03030"))],
            "the value's own spelling, where a name stood before"
        );
        let spaced: Vec<&str> = rows
            .iter()
            .filter_map(|r| r.line_height.as_deref())
            .collect();
        assert_eq!(spaced, ["1.3"]);

        assert_eq!(d.font_size_at_caret().as_deref(), Some("14pt"));
        assert_eq!(d.font_family_at_caret().as_deref(), Some("Garamond"));
        assert_eq!(d.text_color_at_caret().as_deref(), Some("#c03030"));
        assert_eq!(d.line_spacing_at_caret().as_deref(), Some("1.3"));

        // A spelling that means the same value is written back the one way: a
        // ratio that spells a name is that name, and `#rgb` expands.
        in_text(&mut d);
        assert!(d.set_line_spacing(Some("1.50".into())).is_ok());
        in_text(&mut d);
        assert_eq!(d.line_spacing_at_caret().as_deref(), Some("1.5"));
        assert!(d.set_text_color(Some("#f00".into())).is_ok());
        in_text(&mut d);
        assert_eq!(d.text_color_at_caret().as_deref(), Some("#ff0000"));
    }

    /// A named face reaching a run **inside a table cell** — the other road a
    /// run takes to a renderer, and the one an id resolved against the wrong
    /// table would quietly ruin: a cell's runs come through [`cell_lines`] and
    /// not through [`wysiwyg_rows`], so the map's [`CoreFaceTable`] has to be
    /// threaded down both. A grid whose faces all named nothing would draw in
    /// the page's own face and look like a machine that simply had no Garamond.
    #[test]
    fn a_named_face_reaches_a_run_inside_a_table_cell() {
        let mut d = handle("| a | b |\n|---|---|\n| one | two |\n");
        let at = d.doc.source.find("one").unwrap();
        d.doc.place_caret(at, false);
        d.doc.place_caret(at + 3, true);
        assert!(d.set_font_family(Some("Garamond".into())).is_ok());

        let tables = wysiwyg_tables(&d.doc.vmap, usize::MAX, usize::MAX, &[]);
        let faced: Vec<(String, String)> = tables
            .iter()
            .flat_map(|t| t.grid.iter())
            .flat_map(|r| r.cells.iter())
            .flat_map(|c| c.lines.iter())
            .flat_map(|l| l.runs.iter())
            .filter_map(|r| Some((r.text.trim().to_string(), r.font.clone()?)))
            .collect();
        assert_eq!(faced, [("one".to_string(), "Garamond".to_string())]);
    }

    /// Each vocabulary reads back the name the document carries — and now the
    /// value beside it, since this surface speaks strings both ways and the
    /// grammar is the document's own. A token outside that grammar is refused
    /// rather than read as a clearing: the argument that clears is `null`, and
    /// the two differ by one typo.
    #[test]
    fn a_vocabulary_reads_back_its_own_names_and_its_own_values() {
        use leaf_core::style::{
            FontFamily as CoreFontFamily, LineSpacing as CoreLineSpacing, SizeStep as CoreSizeStep,
        };
        assert_eq!(alignment(Some("center")).unwrap(), Some(CoreAlign::Center));
        assert_eq!(
            line_spacing(Some("1.5")).unwrap(),
            Some(CoreLineHeight::Step(CoreLineSpacing::OneHalf))
        );
        assert_eq!(
            font_size(Some("large")).unwrap(),
            Some(CoreFontSize::Step(CoreSizeStep::Large))
        );
        assert_eq!(
            font_family(Some("monospace")).unwrap(),
            Some(CoreFontFace::Generic(CoreFontFamily::Monospace))
        );
        assert_eq!(
            text_color(Some("blue")).unwrap(),
            Some(CoreTextColor::Named(CoreMarkColor::Blue))
        );
        // And the exact forms, each spelled the one canonical way back.
        assert_eq!(font_size(Some("14pt")).unwrap(), CoreFontSize::points(14.0));
        assert_eq!(
            line_spacing(Some("1.3")).unwrap(),
            CoreLineHeight::ratio(1.3)
        );
        assert_eq!(
            font_family(Some("Garamond")).unwrap(),
            Some(CoreFontFace::Named("Garamond".to_string()))
        );
        assert_eq!(
            text_color(Some("#c03030")).unwrap(),
            Some(CoreTextColor::Rgb {
                r: 0xc0,
                g: 0x30,
                b: 0x30
            })
        );
        // Absence is the theme's own, and `null` is how to ask for it.
        assert_eq!(alignment(None).unwrap(), None);
        assert_eq!(line_spacing(None).unwrap(), None);
        assert_eq!(font_size(None).unwrap(), None);
        assert_eq!(font_family(None).unwrap(), None);
        assert_eq!(text_color(None).unwrap(), None);
        // And so is `"1"`, the one token in a grammar here that *means* the
        // theme's own: single spacing has no token, so it clears rather than
        // being refused — which is what the gesture's own note promises.
        assert_eq!(line_spacing(Some("1")).unwrap(), None);
        assert_eq!(line_spacing(Some("1.0")).unwrap(), None);
        assert_eq!(line_spacing(Some(" 1.00 ")).unwrap(), None);
        // A token outside the grammar is an error rather than a clearing —
        // which each helper spells by turning the `None` below into an `Err`.
        // Asked of the core vocabularies rather than of the helpers, because
        // building that error is `JsValue::from_str`, which panics off wasm32
        // and would abort the run rather than fail a case.
        assert_eq!(CoreAlign::from_token("left"), None);
        assert_eq!(CoreLineHeight::from_attr("1"), None, "single is absence");
        assert_eq!(CoreFontSize::from_attr("medium"), None, "and so is medium");
        assert_eq!(CoreFontSize::from_attr("huge"), None);
        assert_eq!(
            CoreFontSize::from_attr("14px"),
            None,
            "a document is not a screen"
        );
        assert_eq!(
            CoreFontFace::from_attr("   "),
            None,
            "a name that names nothing"
        );
        assert_eq!(CoreTextColor::from_attr("rgb(1, 2, 3)"), None);
    }

    /// A page break crosses as the leaf directive it is — the row a paginating
    /// renderer opens a page at, and a dashed rule for one that scrolls.
    #[test]
    fn a_page_break_crosses_as_a_directive_of_its_own() {
        let mut d = handle("before\n\nafter\n");
        assert!(d.insert_page_break().is_ok());
        let names: Vec<String> = wysiwyg_directives(&d.doc.vmap)
            .into_iter()
            .map(|x| x.name)
            .collect();
        assert_eq!(names, ["page-break"]);
        assert!(d.doc.source.contains("page-break"));
    }

    /// One flag per new control, answered by the format — the toolbar builds
    /// itself from these rather than discovering each refusal on a press.
    /// Markdown spells all six; XML spells none of them, being parse-only.
    #[test]
    fn capabilities_answer_for_the_presentation_controls_too() {
        let md = CapabilitiesView::from(handle("x\n").doc.capabilities());
        assert!(md.alignment && md.line_spacing);
        assert!(md.font_size && md.font_family && md.text_color);
        assert!(md.page_break);

        let xml = LeafDoc::new("<a>x</a>", "xml").expect("xml parses");
        let xml = CapabilitiesView::from(xml.doc.capabilities());
        assert!(!xml.alignment && !xml.line_spacing);
        assert!(!xml.font_size && !xml.font_family && !xml.text_color);
        assert!(!xml.page_break);
    }

    #[test]
    fn the_caret_knows_whether_it_is_in_a_table() {
        let mut d = handle("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let cell = d.doc.source.find(" 1 ").unwrap() + 1;
        d.doc.place_caret(cell, false);
        assert!(d.caret_in_table());

        let mut plain = handle("just a paragraph\n");
        assert!(!plain.caret_in_table());
    }

    /// Unwrapped means no line folds, however long the document's longest is.
    #[test]
    fn unwrapped_folds_nothing() {
        let long = format!("{}\n", "word ".repeat(200));
        let mut d = handle(&long);
        d.width = 40;
        d.sync();
        let folded = d.doc.vmap.rows.len();
        d.width = usize::MAX;
        d.sync();
        assert!(d.doc.vmap.rows.len() < folded);
    }

    /// A document with no table publishes no grid — so a renderer's "skip these
    /// rows" set is empty and it paints every row, as it always did.
    #[test]
    fn a_document_without_a_table_publishes_no_grid() {
        let doc = wysiwyg("# just a heading\n\nand a paragraph.\n");
        assert!(wysiwyg_tables(&doc.vmap, usize::MAX, usize::MAX, &[]).is_empty());
        assert!(wysiwyg_directives(&doc.vmap).is_empty());
    }

    /// The whole point of the record is that the numbers cross the boundary,
    /// so this checks the ones a host would show — and that a selection
    /// narrows them and no selection answers nothing at all.
    #[test]
    fn counts_cross_the_boundary_whole_and_selected() {
        let mut d = LeafDoc::new("a **bold** word\n\n- item\n", "markdown").unwrap();
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
        // Straight onto the caret and anchor: `select_range` would hand back a
        // `DocView`, which is a frame for a browser to paint and not a thing
        // to build off wasm.
        d.doc.anchor = Some(0);
        d.doc.caret = 10;
        let s = d.selection_counts().expect("a selection");
        assert_eq!((s.words, s.characters, s.paragraphs), (2, 6, 1));
    }

    #[test]
    fn a_formula_is_a_math_run_and_a_view_paired_by_src() {
        let mut doc = wysiwyg("say $x+y$ here\n\nnext\n");
        doc.set_inline_pictures(true);
        doc.caret = 16;
        doc.build_visual(80);
        let views = math_views(&doc.vmap);
        assert_eq!(views.len(), 1);
        let m = &views[0];
        assert!(m.inline);
        assert_eq!((m.row, m.rows), (0, 1));
        assert_eq!(m.tex, "x+y");
        assert!(!m.display);
        assert_eq!(m.src, 4);
        let rows = wysiwyg_rows(&doc.vmap, usize::MAX, usize::MAX, &[]);
        let run = rows[0]
            .runs
            .iter()
            .find(|r| r.role == "math")
            .expect("a math run");
        assert_eq!(run.text, "∑");
        assert_eq!(run.src, m.src);
        // A display block is rows to lay over, grown by what was measured.
        let mut doc = wysiwyg("$$\nx\n$$\n\nend\n");
        doc.caret = 9;
        doc.build_visual(80);
        let m = &math_views(&doc.vmap)[0];
        assert!(!m.inline && m.display);
        assert_eq!((m.row, m.rows), (0, 1));
        doc.set_math_rows([(m.tex.clone(), 3)].into_iter().collect());
        doc.build_visual(80);
        assert_eq!(math_views(&doc.vmap)[0].rows, 3);
    }

    // ── frames as changes ────────────────────────────────────────────────

    /// A `LeafDoc` built off wasm: the constructor installs the panic hook,
    /// which wants a JS console, so the struct is put together by hand here.
    fn binding(source: &str) -> LeafDoc {
        LeafDoc {
            doc: Doc::from_source(source.to_string(), Format::Markdown).unwrap(),
            width: 80,
            scheme: ColorScheme::Light,
            incremental: false,
            last_rows: None,
            frame: 0,
        }
    }

    /// Apply `frame`, a change, to `rows` — what `editor.js` does with one.
    fn apply(rows: &mut Vec<Row>, frame: &DocView) {
        let d = leaf_core::RowDelta {
            start: frame.row_start,
            replaced: frame.replaced,
            len: frame.rows.len(),
            src_shift: frame.src_shift as i64,
        };
        leaf_core::apply_row_delta(rows, d, &frame.rows, |row, by| {
            for run in &mut row.runs {
                run.src = (run.src as i64 + by) as usize;
            }
        });
        assert_eq!(rows.len(), frame.row_count);
    }

    #[test]
    fn changes_applied_in_order_are_the_whole_frame_and_a_click_lifts_none() {
        let src: String = (0..40)
            .map(|i| format!("paragraph {i} with `code` and **bold** words in it\n\n"))
            .collect::<Vec<_>>()
            .concat()
            + "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter $x$ math\n";
        let mut a = binding(&src);
        let mut b = binding(&src);
        a.set_incremental_frames(true);
        let first = a.set_unwrapped().unwrap();
        b.set_unwrapped().unwrap();
        assert_eq!(first.basis, 0);
        let mut rows = first.rows.clone();
        let n = rows.len();

        let moved = a.click_ch(n / 2, 3, false).unwrap();
        b.click_ch(n / 2, 3, false).unwrap();
        assert_eq!(moved.basis, first.frame);
        assert!(moved.rows.is_empty(), "a click lifts no row");
        apply(&mut rows, &moved);

        let typed = a.insert("x").unwrap();
        let whole = b.insert("x").unwrap();
        assert_eq!(typed.rows.len(), 1, "a keystroke lifts the row it changed");
        assert_eq!(typed.src_shift, 1);
        apply(&mut rows, &typed);
        assert!(rows == whole.rows);

        let mut seed = 7u64;
        for step in 0..60 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let extend = (seed >> 40).is_multiple_of(3);
            let (fa, fb) = match (seed >> 33) % 8 {
                0 => (a.insert("y").unwrap(), b.insert("y").unwrap()),
                1 => (a.backspace().unwrap(), b.backspace().unwrap()),
                2 => (a.newline().unwrap(), b.newline().unwrap()),
                3 => (a.move_up(extend).unwrap(), b.move_up(extend).unwrap()),
                4 => (a.move_down(extend).unwrap(), b.move_down(extend).unwrap()),
                5 => (a.toggle_bold().unwrap(), b.toggle_bold().unwrap()),
                6 => (a.undo().unwrap(), b.undo().unwrap()),
                _ => (
                    a.move_word_right(extend).unwrap(),
                    b.move_word_right(extend).unwrap(),
                ),
            };
            apply(&mut rows, &fa);
            assert!(rows == fb.rows, "step {step}");
        }
        let v = a.view().unwrap();
        assert_eq!(v.basis, 0);
        assert!(v.rows == rows);
        assert_eq!(a.rows(2, 4), rows[2..4].to_vec());
    }

    #[test]
    fn frames_are_whole_unless_asked_for() {
        let mut d = binding("one\n\ntwo\n");
        let v = d.set_unwrapped().unwrap();
        assert_eq!((v.frame, v.basis, v.row_count), (1, 0, 3));
        let v = d.move_right(false).unwrap();
        assert_eq!((v.frame, v.basis, v.rows.len()), (2, 0, 3));
        d.set_incremental_frames(true);
        assert_eq!(d.move_right(false).unwrap().basis, 0, "the first is whole");
        let v = d.move_right(false).unwrap();
        assert_eq!((v.basis, v.rows.len()), (3, 0));
        d.set_incremental_frames(false);
        assert_eq!(d.move_right(false).unwrap().rows.len(), 3);
    }
}
