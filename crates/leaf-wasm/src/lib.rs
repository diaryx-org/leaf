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

use leaf_core::style::{Baseline, Role, Style as LStyle};
use leaf_core::wysiwyg::text_width;
use leaf_core::{
    Alignment, BlockClass, BlockKind, ColorScheme, Doc, Format, Glyph, Highlight as CoreHighlight,
    InlineKind, LineFlow as CoreLineFlow, MarkupMode as CoreMarkupMode, MediaKind, View, VisualMap,
};
use serde::{Deserialize, Serialize};
use tsify_next::Tsify;
use unicode_segmentation::UnicodeSegmentation;
use wasm_bindgen::prelude::*;

/// One maximal span of same-styled glyphs on a visual row — the unit the JS
/// renderer turns into a single styled DOM node.
#[derive(Serialize, Tsify)]
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
#[derive(Serialize, Tsify)]
pub struct BoundaryView {
    above: String,
    below: String,
}

/// One visual line: its styled runs plus the row-level flags a frontend draws
/// chrome from.
#[derive(Serialize, Tsify)]
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
    /// Covers `insertMedia` too.
    image: bool,
    thematic_break: bool,
    /// The footnote button — writes the `[^1]` and the definition it needs.
    footnote: bool,
    code_language: bool,
    /// The grid controls. Gate them on this *and* `caretInTable`: this asks
    /// whether the format's tables are editable, that whether the caret is in
    /// one — an HTML `<table>` answers yes to the second and no to the first.
    table: bool,
    /// Shift+Return inside a cell.
    cell_line_break: bool,
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
            image: c.image,
            thematic_break: c.thematic_break,
            footnote: c.footnote,
            code_language: c.code_language,
            table: c.table,
            cell_line_break: c.cell_line_break,
        }
    }
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

/// A whole rendered frame: the rows to paint, where the caret sits, and the
/// toolbar state — everything the JS side needs for one repaint, in one object.
///
/// `into_wasm_abi` makes this the *return type* of every view-producing method:
/// the generated `.d.ts` types those methods as `DocView` rather than `any`, so
/// the JS renderer sees the full shape.
#[derive(Serialize, Tsify)]
#[tsify(into_wasm_abi)]
pub struct DocView {
    rows: Vec<Row>,
    /// Tables described structurally, for the proportional renderer that draws
    /// its own grid instead of painting the box-glyph rows. Empty in the source
    /// view. Each names the `rows` span its picture occupies, to be skipped.
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
    /// history buttons enable by. Both false on a read-only document.
    can_undo: bool,
    can_redo: bool,
    /// `"wysiwyg"` or `"source"`, for a view-toggle affordance.
    view: String,
    /// The heading level at the caret, if any — a toolbar lights H1…H6 from it.
    heading: Option<u32>,
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
    /// Every block-level image, video, and audio in the frame, in row order —
    /// the placeholder rows the renderer replaces with real elements. Empty in
    /// the source view, which shows the markup itself and has no placeholders.
    media: Vec<MediaView>,
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
        })
    }

    /// Rebuild the visual map at the current width. Cheap (cached) when nothing
    /// changed; the guard that lets every movement/click method assume a fresh
    /// grid regardless of the order JS calls them in.
    fn sync(&mut self) {
        self.doc.build_visual(self.width);
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

    /// Resolve the current document to a renderable frame of style runs. Called
    /// for the first paint, on resize, and returned by every mutating method so
    /// one boundary crossing both edits and repaints.
    pub fn view(&mut self) -> Result<DocView, JsValue> {
        self.sync();

        let (ss, se) = self.doc.selection().unwrap_or((usize::MAX, usize::MAX));

        // The two views speak different grids — the WYSIWYG map's resolved glyphs
        // vs the raw source split on newlines — and `caret_pos` below already
        // branches to match, so the rows must too or the caret lands on the wrong
        // text. See `Doc::caret_pos`.
        let rows = match self.doc.view {
            View::Wysiwyg => wysiwyg_rows(&self.doc.vmap, ss, se, self.doc.highlights()),
            View::Source => source_rows(&self.doc.source, ss, se),
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
        // Read before the frame is assembled: it needs `&mut self`, which the
        // struct literal's other fields are already borrowing out of.
        let link = self.doc.link_destination_at_caret();
        let active = self
            .doc
            .active_inline_marks()
            .iter()
            .map(|k| mark_id(k).to_string())
            .collect();

        Ok(DocView {
            rows,
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
            active,
            caret_src: self.doc.caret,
            link,
            // Only the WYSIWYG view has placeholder rows to replace; the source
            // view is the markup itself, where a `<video>` tag *is* the content.
            media: match self.doc.view {
                View::Wysiwyg => media_views(&self.doc.vmap, self.scheme),
                View::Source => Vec::new(),
            },
        })
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
        self.view()
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
        self.view()
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
        self.view()
    }

    /// Set the wrap width (in columns) the viewport implies and repaint.
    pub fn set_width(&mut self, cols: usize) -> Result<DocView, JsValue> {
        self.width = cols.max(1);
        self.view()
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
        self.view()
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
        self.view()
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
    /// quote and link, and Markdown refuses the highlight djot spells.
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
        self.view()
    }

    // ── text input ──────────────────────────────────────────────────────────

    pub fn insert(&mut self, text: &str) -> Result<DocView, JsValue> {
        self.doc.insert(text);
        self.view()
    }

    pub fn paste(&mut self, text: &str) -> Result<DocView, JsValue> {
        self.doc.paste(text);
        self.view()
    }

    pub fn newline(&mut self) -> Result<DocView, JsValue> {
        self.doc.newline();
        self.view()
    }

    /// Tab: indent the caret's line (or the selected lines) one level, nesting a
    /// list item under its sibling.
    pub fn indent(&mut self) -> Result<DocView, JsValue> {
        self.doc.indent();
        self.view()
    }

    /// Shift+Tab: take one indent level back off the caret's line (or the
    /// selected lines), unnesting a list item.
    pub fn outdent(&mut self) -> Result<DocView, JsValue> {
        self.doc.outdent();
        self.view()
    }

    pub fn backspace(&mut self) -> Result<DocView, JsValue> {
        self.doc.backspace();
        self.view()
    }

    pub fn delete_forward(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_forward();
        self.view()
    }

    pub fn delete_word_back(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_word_back();
        self.view()
    }

    pub fn delete_word_forward(&mut self) -> Result<DocView, JsValue> {
        self.doc.delete_word_forward();
        self.view()
    }

    // ── caret movement ──────────────────────────────────────────────────────
    // Each syncs the grid first (movement reads the stop table / column layout),
    // moves, then repaints.

    pub fn move_left(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_left(extend);
        self.view()
    }

    pub fn move_right(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_right(extend);
        self.view()
    }

    pub fn move_up(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_up(extend);
        self.view()
    }

    pub fn move_down(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_down(extend);
        self.view()
    }

    pub fn move_word_left(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_word_left(extend);
        self.view()
    }

    pub fn move_word_right(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_word_right(extend);
        self.view()
    }

    pub fn move_home(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_home(extend);
        self.view()
    }

    pub fn move_end(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_end(extend);
        self.view()
    }

    pub fn move_doc_start(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_doc_start(extend);
        self.view()
    }

    pub fn move_doc_end(&mut self, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.move_doc_end(extend);
        self.view()
    }

    pub fn select_all(&mut self) -> Result<DocView, JsValue> {
        self.doc.select_all();
        self.view()
    }

    /// Place the caret from a click, in core's column grid: `row` indexes the
    /// visual [`Row`]s and `col` is the glyph column within it. A proportional
    /// renderer derives them by hit-testing — `caretRangeFromPoint` gives the DOM
    /// node+offset under the pointer, which maps to a row and a column count. Core
    /// clamps both to real caret stops.
    pub fn click(&mut self, row: usize, col: usize, extend: bool) -> Result<DocView, JsValue> {
        self.sync();
        self.doc.click(row, col, extend);
        self.view()
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
        self.view()
    }

    /// The source offset under a click at row `row`, `ch` UTF-16 units in — the
    /// same resolution [`click_ch`] does, but returning the offset instead of
    /// moving the caret. It's what the double/triple-click selectors below anchor
    /// on, and it lets a host implement its own gestures (a context menu placing
    /// the caret, say) without a second boundary crossing.
    fn offset_at(&mut self, row: usize, ch: usize) -> usize {
        self.sync();
        let col = utf16_to_col(&self.row_text(row), ch);
        self.doc.click(row, col, false);
        self.doc.caret
    }

    /// Select the word under a click (row, `ch`) — the double-click gesture.
    /// Core reads the word from the source around that offset.
    pub fn select_word_ch(&mut self, row: usize, ch: usize) -> Result<DocView, JsValue> {
        let off = self.offset_at(row, ch);
        self.doc.select_word_at(off);
        self.view()
    }

    /// Select the whole logical text block under a click (row, `ch`) — the
    /// triple-click gesture. Core reads the paragraph/heading span from the AST,
    /// so it grabs the entire block even where it soft-wraps across visual rows.
    pub fn select_block_ch(&mut self, row: usize, ch: usize) -> Result<DocView, JsValue> {
        let off = self.offset_at(row, ch);
        self.doc.select_block_at(off);
        self.view()
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
        self.view()
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
        self.view()
    }

    // ── formatting commands (mirror leaf-gpui's EditorCommand) ───────────────

    pub fn toggle_bold(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Strong);
        self.view()
    }

    pub fn toggle_italic(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Emph);
        self.view()
    }

    pub fn toggle_code(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Verbatim);
        self.view()
    }

    pub fn toggle_mark(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Mark);
        self.view()
    }

    pub fn toggle_underline(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Insert);
        self.view()
    }

    pub fn toggle_strike(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle(InlineKind::Delete);
        self.view()
    }

    pub fn set_paragraph(&mut self) -> Result<DocView, JsValue> {
        self.doc.set_block(BlockKind::Paragraph);
        self.view()
    }

    /// Toggle the current block to a heading of `level` (1–6); toggling the
    /// active level off returns it to a paragraph, per core.
    pub fn set_heading(&mut self, level: u32) -> Result<DocView, JsValue> {
        self.doc.toggle_heading(level);
        self.view()
    }

    pub fn toggle_blockquote(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_blockquote();
        self.view()
    }

    pub fn toggle_list(&mut self, ordered: bool) -> Result<DocView, JsValue> {
        self.doc.toggle_list(ordered);
        self.view()
    }

    /// Tick or untick the task item at the caret.
    pub fn toggle_task_checked(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_task_checked();
        self.view()
    }

    /// Tick or untick the task item covering `offset` — a click on a rendered
    /// checkbox, which leaves the caret where it was.
    pub fn toggle_task_at(&mut self, offset: usize) -> Result<DocView, JsValue> {
        self.doc.toggle_task_at(offset);
        self.view()
    }

    /// Give the list item at the caret a checkbox, or take its checkbox away.
    pub fn toggle_task_item(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_task_item();
        self.view()
    }

    /// Whether the item at the caret has a box, and which way it faces.
    pub fn task_checked_at_caret(&mut self) -> Option<bool> {
        self.doc.task_checked_at_caret()
    }

    pub fn insert_link(&mut self, destination: &str) -> Result<DocView, JsValue> {
        self.doc.insert_link(destination);
        self.view()
    }

    pub fn undo(&mut self) -> Result<DocView, JsValue> {
        self.doc.undo();
        self.view()
    }

    pub fn redo(&mut self) -> Result<DocView, JsValue> {
        self.doc.redo();
        self.view()
    }

    /// Switch between the rendered WYSIWYG surface and the raw source.
    pub fn toggle_view(&mut self) -> Result<DocView, JsValue> {
        self.doc.toggle_view();
        self.view()
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
        self.view()
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
        self.view()
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
        self.view()
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
        self.view()
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
        self.view()
    }

    /// Stop wrapping: lay every logical line out unbroken, for a surface that
    /// scrolls horizontally instead of folding.
    pub fn set_unwrapped(&mut self) -> Result<DocView, JsValue> {
        // Core takes a column budget, not an option, so "unwrapped" is a budget
        // no line can reach.
        self.width = usize::MAX;
        self.view()
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

    /// Where a locator lands — the span of the block a fragment id names, for
    /// following an in-document link.
    pub fn locate(&mut self, id: &str) -> Option<LandingView> {
        self.doc.locate(id).map(LandingView::from)
    }

    /// Write a footnote reference at the caret and the definition it needs.
    pub fn insert_footnote(&mut self) -> Result<DocView, JsValue> {
        self.doc.insert_footnote();
        self.view()
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
        self.view()
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
        self.view()
    }

    pub fn table_delete_row(&mut self) -> Result<DocView, JsValue> {
        self.doc.table_delete_row();
        self.view()
    }

    pub fn table_insert_column(&mut self, right: bool) -> Result<DocView, JsValue> {
        self.doc.table_insert_column(right);
        self.view()
    }

    pub fn table_delete_column(&mut self) -> Result<DocView, JsValue> {
        self.doc.table_delete_column();
        self.view()
    }

    pub fn table_move_row(&mut self, down: bool) -> Result<DocView, JsValue> {
        self.doc.table_move_row(down);
        self.view()
    }

    pub fn table_move_column(&mut self, right: bool) -> Result<DocView, JsValue> {
        self.doc.table_move_column(right);
        self.view()
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
        self.view()
    }

    /// Tab inside a table: to the next (or previous) cell, adding a row when it
    /// steps off the end. `undefined` when the caret isn't in one, so the host
    /// can fall through to its ordinary Tab (indent).
    pub fn cell_tab(&mut self, forward: bool) -> Result<Option<DocView>, JsValue> {
        if self.doc.cell_tab(forward) {
            self.view().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Return inside a table: to the cell below, adding a row at the last one.
    /// `undefined` when the caret isn't in a table.
    pub fn cell_return(&mut self) -> Result<Option<DocView>, JsValue> {
        if self.doc.cell_return() {
            self.view().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Shift+Return inside a cell: a line break *within* the cell rather than a
    /// new row. `undefined` when the caret isn't in a table.
    pub fn cell_line_break(&mut self) -> Result<Option<DocView>, JsValue> {
        if self.doc.cell_line_break() {
            self.view().map(Some)
        } else {
            Ok(None)
        }
    }
}

/// The WYSIWYG rows: each visual row's glyphs coalesced into maximal runs of
/// identical `(style, selected)` — the same span merge the TUI does. A glyph is
/// selected when its source byte lies in `[ss, se)`.
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
                runs: runs_of(&vrow.glyphs, ss, se, hls),
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
            }
        })
        .collect()
}

/// The source rows: the raw document split on `'\n'`, every line plain body text
/// with the `[ss, se)` selection carved out as its own run — the browser
/// counterpart of the TUI's `build_lines`. This is what backs the source view,
/// whose caret rides raw byte offsets (see `Doc::caret_pos`).
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
                runs.push(make_run(raw[..a].to_string(), body, false, None, 0));
            }
            runs.push(make_run(raw[a..b].to_string(), body, true, None, 0));
            if b < raw.len() {
                runs.push(make_run(raw[b..].to_string(), body, false, None, 0));
            }
        } else if !raw.is_empty() {
            runs.push(make_run(raw.to_string(), body, false, None, 0));
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
        });
        byte = end + 1; // skip the '\n' that `split` consumed
    }
    rows
}

/// Build a [`Run`] from an accumulated string and the core style it was drawn
/// with — the one place role, emphasis, and baseline cross into the view shape.
fn make_run(text: String, style: LStyle, sel: bool, hl: Option<&CoreHighlight>, src: usize) -> Run {
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

/// Coalesce `glyphs` into maximal runs of identical `(style, selected)` — the
/// shared body of a row's runs and a table cell's. A glyph is selected when its
/// source byte lies in `[ss, se)`, so the selection splits a run exactly as a
/// style change does.
fn runs_of(glyphs: &[Glyph], ss: usize, se: usize, hls: &[CoreHighlight]) -> Vec<Run> {
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
                runs: runs_of(&seg, ss, se, hls),
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
        runs: runs_of(&seg, ss, se, hls),
        start: line_start.unwrap_or(cell_end),
        end: cell_end,
    });
    lines
}

/// The structural tables of a WYSIWYG frame — each with the `rows` span its
/// box-glyph picture occupies (to be skipped) and its grid of styled cells.
fn wysiwyg_tables(vmap: &VisualMap, ss: usize, se: usize, hls: &[CoreHighlight]) -> Vec<TableView> {
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
                            lines: cell_lines(&cell.glyphs, cell.start, cell.end, ss, se, hls),
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

    #[test]
    fn runs_coalesce_by_style_and_split_on_the_selection_edge() {
        let doc = wysiwyg("plain **bold** plain\n");
        let glyphs = &doc.vmap.rows[0].glyphs;

        let runs = runs_of(glyphs, usize::MAX, usize::MAX, &[]);
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
        let split = runs_of(glyphs, start, start + 2, &[]);
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
            .flat_map(|r| runs_of(&r.glyphs, usize::MAX, usize::MAX, &[]))
            .filter(|r| r.sup)
            .map(|r| r.text.clone())
            .collect();
        assert!(!sup.is_empty(), "no run came across raised: {sup:?}");
        assert!(sup.iter().all(|t| !t.is_empty()));
        assert!(
            doc.vmap
                .rows
                .iter()
                .flat_map(|r| runs_of(&r.glyphs, usize::MAX, usize::MAX, &[]))
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
            .flat_map(|r| runs_of(&r.glyphs, usize::MAX, usize::MAX, &[]))
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
    fn a_link_answers_at_its_own_offset_and_nowhere_else() {
        let mut d = handle("see [the docs](https://example.com/x) now\n");
        let inside = d.doc.source.find("the docs").unwrap() + 2;
        assert_eq!(
            d.link_destination_at(inside).as_deref(),
            Some("https://example.com/x")
        );
        assert!(d.link_destination_at(0).is_none());
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
}
