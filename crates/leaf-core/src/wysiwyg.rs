//! The WYSIWYG view: render the document with its markup *resolved*, not shown —
//! headings and code tagged with a typographic role (a frontend sizes or colours
//! them; see [`crate::style`]), `**bold**` as real bold, `# ` / `**` / `` ` ``
//! delimiters hidden — while keeping every visible glyph tied back to the source
//! byte it came from.
//!
//! That back-reference (`Glyph::src`) is what lets a caret still work: the caret
//! stays a source offset (shared with the source view), but the [`VisualMap`]
//! converts between an offset and a screen `(row, col)`, so cursor drawing,
//! mouse clicks, and vertical motion all operate in *visible* space.
//!
//! Left and Right instead walk the map's caret *stops* in document order. On
//! ordinary prose that's the same journey — the stops are laid out left to right
//! — and it steps over the hidden delimiters either way. They part company only
//! in a table, where the text is arranged in two dimensions and a cell wrapped
//! within its column continues *below* rather than to the right. Following the
//! document is what a caret means there.
//!
//! Text is walked from the AST (`str` nodes carry exact spans, and their text is
//! the verbatim source slice), so a Markdown and a Djot file that parse alike
//! render — and map — identically.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;

use twig::{Alignment, ContainerOrigin, DirectiveForm, Editor, FlatNode, Kind, QueryMatch};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::style::{Baseline, Role, Style};

/// One rendered character plus the source byte offset it originates from.
/// Synthetic glyphs (a list bullet, a quote gutter) point at their block's
/// start, so clicking one lands the caret at the start of that block.
#[derive(Clone)]
pub struct Glyph {
    pub ch: char,
    pub style: Style,
    pub src: usize,
    /// Whether the caret may *rest* on this glyph. Decoration — a table border
    /// or a cell's alignment padding — is visible but isn't text, so the caret
    /// steps over it instead of into it. It also can't be a stop even in
    /// principle: a run of decoration shares one `src`, and a caret can only
    /// move by changing offset, so resting on it would pin horizontal motion.
    /// A click still maps through `src`, which is why decoration points at the
    /// text it decorates.
    ///
    /// Real text is a stop once per *grapheme cluster*, on the glyph that opens
    /// it: the continuation glyphs of an emoji or an accented letter are drawn,
    /// but standing between them is standing inside a character.
    pub stop: bool,
}

/// One visual line. `end_src` is the source offset a caret sits at when placed
/// at the line's end (past its last glyph) — the anchor for end-of-line and
/// click-past-content.
///
/// `Clone` so a block's rows can be cached and re-emitted at a shifted offset
/// across an edit — see [`BlockCache`].
#[derive(Clone)]
pub struct VRow {
    pub glyphs: Vec<Glyph>,
    pub end_src: usize,
    /// A row that is drawn but holds no caret: a table's `├───┼───┤` rules, and
    /// the blank gap a block boundary is spelled with. Vertical motion steps
    /// over it, `pos_of_offset` never resolves onto it, and its stops (it has
    /// none) and `end_src` stay out of the map's stop table.
    ///
    /// Emptiness isn't the test — an empty paragraph is a blank row too, and a
    /// real caret stop. The test is whether the row is somewhere text can go.
    pub decoration: bool,
    /// This row is one line of a fenced or indented code block. Set on every row
    /// the `"code_block"` arm emits — including its blank lines, which carry no
    /// glyph to tell them apart otherwise. A frontend draws its own chrome (a
    /// border and a tinted background) around each maximal run of these, and
    /// scrolls them horizontally instead of wrapping; see
    /// [`VisualMap::code_blocks`]. Survives the row shuffling of [`BlockCache`]
    /// reuse and [`build_spliced`] because it rides on the row, not on a
    /// row-index span the way a table's picture does.
    pub code: bool,
    /// A fenced code block's info string (its language), carried on the *first*
    /// row of the block so it survives row reuse the way [`code`](Self::code)
    /// does. `None` on every other row, and on an indented block (which has no
    /// fence to label). A frontend paints it as a small label on the block's box
    /// and edits it through a prompt — see [`CodeBlockInfo::lang`]. It's a plain
    /// display string, not a source slice, so it needs no offset shifting; the
    /// label re-derives from twig on the next build.
    pub code_lang: Option<String>,
    /// This row belongs to a `:::name{.class}` directive container — twig's
    /// generic fenced-div block, whose meaning is entirely up to the host app
    /// (diaryx's `:::vis{.audience}` visibility blocks, say). Set on every row
    /// the `"directive"` arm emits, the same way [`code`](Self::code) marks a
    /// code block's rows, so a frontend can draw a tinted panel around each
    /// maximal run of these.
    pub directive: bool,
    /// A directive container's space-joined attrs — dot-prefixed classes
    /// (`.public .family` → `"public family"`) unioned with bare pandoc-style
    /// words (`public family`, no leading dot — diaryx's other `:::vis{...}`
    /// convention), carried on the block's *first* row only — the
    /// [`code_lang`](Self::code_lang) pattern. `None` on every other row, and
    /// when the directive carries no such attrs. A frontend paints it as a
    /// small label on the block's panel; it's a plain display string, not a
    /// source slice, so it rides row reuse untouched.
    pub directive_label: Option<String>,
    /// Set on the single placeholder row a block-level image renders to, carrying
    /// the image's destination and alt text; `None` on every other row. The row's
    /// glyphs are the default `🖼 alt` label (which a plain surface paints as-is);
    /// an image-capable frontend reads this to paint the real picture instead,
    /// skipping the row named by [`MediaInfo::rows_span`]. Like
    /// [`code_lang`](Self::code_lang) it's plain display strings, not source
    /// slices, so it rides row reuse and needs no offset shifting; the map's
    /// [`images`](VisualMap::images) side-table is derived from it once the rows
    /// are final, the same way [`code_blocks`](VisualMap::code_blocks) is.
    pub media: Option<MediaMark>,
    /// Set on the **first** row of a task list item, carrying whether its box is
    /// ticked; `None` on every other row, including a plain `list_item`'s. The
    /// row's glyphs already draw the box as `☐ `/`☑ ` in the marker's place, so a
    /// plain surface needs nothing further; a GUI reads this to paint a real
    /// checkbox widget and to know which way it is facing.
    ///
    /// A `bool` rather than a source span, for the reason
    /// [`code_lang`](Self::code_lang) is a plain string: it rides [`BlockCache`]
    /// reuse and [`build_spliced`] untouched, needing no offset shifting. To
    /// *toggle* the box, a frontend maps its click to a source offset the way it
    /// maps any other — the marker's glyphs carry the item's own `src` — and
    /// hands that to [`crate::Doc::toggle_task_at`].
    pub task: Option<bool>,
    /// Set on the single placeholder row a **leaf** directive (`::name{…}`)
    /// renders to, carrying its name and attributes; `None` on every other row.
    /// The container form isn't this — it wraps real blocks and marks each of
    /// them [`directive`](Self::directive) instead. Like [`image`](Self::image)
    /// it's plain display strings, so it rides row reuse untouched, and the map's
    /// [`directives`](VisualMap::directives) side-table is derived from it once
    /// the rows are final.
    pub leaf_directive: Option<DirectiveMark>,
    /// The heading level (1–6) of the block this row belongs to, on every row a
    /// `heading` emits (a long one wraps to several) and `None` everywhere else.
    ///
    /// A frontend that sizes a whole line — a proportional renderer giving the
    /// row a bigger line box — needs the level *per row*, and the glyphs can't
    /// always supply it: an empty heading (`# ` with nothing typed after it,
    /// which is what the toolbar's H1 leaves on a blank line) has no glyph to
    /// carry a [`Role::Heading`] at all, so a glyph scan called it body text and
    /// the line drew at body height until the first character landed. Riding the
    /// row says it once, for the empty case and the wrapped case alike.
    ///
    /// Per-*glyph* styling still comes from [`Role::Heading`] on the glyphs; this
    /// is the row-level fact, and the two agree wherever a heading has content —
    /// same `u8` level, clamped the same way [`heading_style`] clamps it.
    pub heading: Option<u8>,
    /// What this row divides, on the blank rows a block boundary is *drawn* with
    /// and `None` on every other row — including the navigable blank lines of
    /// preserve-soft flow, which are somewhere text can go rather than a gap
    /// between blocks. So `boundary.is_some()` is exactly "this row is a drawn
    /// block boundary", the [`decoration`](Self::decoration) rows that come from
    /// [`Builder::emit_separators_before`].
    ///
    /// It exists because a boundary's *height* is a frontend decision but its
    /// *kind* is not. Typography spaces a boundary by what it separates — the
    /// margin above a heading is wider than the one between two paragraphs, so
    /// the heading groups with the text it introduces — and a frontend that has
    /// only rows to look at has to re-derive the structure by sniffing glyph
    /// roles. Three frontends sniffing separately is three chances to disagree
    /// about the same document. Core already knows, having just walked the AST
    /// to emit this row, so it says so once here and each frontend multiplies by
    /// its own spacing.
    pub boundary: Option<Boundary>,
}

/// What a drawn block boundary separates: the kinds of the blocks it falls
/// between — the pair a frontend spaces by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boundary {
    pub above: BlockClass,
    pub below: BlockClass,
}

/// The block kinds core tells apart when it walks a document — the vocabulary
/// [`Boundary`] is spelled in. A statement about *structure*, not about how any
/// of it should look: what a frontend does with "this gap sits above a heading"
/// is entirely the frontend's.
///
/// `Class` rather than `Kind` because [`twig::BlockKind`] already means
/// something else in this crate's public surface — the *command* vocabulary
/// (`Paragraph | Heading(n)`) a toolbar passes to [`Doc::set_block`](crate::Doc::set_block).
/// This is the reverse direction: what a block already *is*, read back off a
/// rendered row.
///
/// [`BlockClass::Other`] is the honest answer for a node kind core doesn't
/// separate out, so adding one here is additive for every frontend: nothing has
/// to change until it wants to space that kind differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockClass {
    Paragraph,
    Heading,
    /// A whole list. Its *items* are [`BlockClass::ListItem`]; note that core
    /// draws no boundary row between two items of one list, tight or loose, so
    /// an item↔item pair never reaches a frontend.
    List,
    ListItem,
    Quote,
    Code,
    Table,
    /// A block-level image, video, or audio.
    Media,
    /// A `:::name{.class}` directive container.
    Directive,
    Rule,
    Footnote,
    Other,
}

impl BlockClass {
    /// Classify a twig node kind — the same vocabulary [`Builder::block`]
    /// matches on, so the two can't drift about what a block is. Both the
    /// whole-arena walk (which has [`FlatNode`]s) and the incremental top-level
    /// walk (which has only a query match's kind) reach it by this one door.
    pub fn from_node_kind(kind: &Kind) -> BlockClass {
        match kind {
            Kind::Para => BlockClass::Paragraph,
            Kind::Heading => BlockClass::Heading,
            Kind::BulletList | Kind::OrderedList | Kind::TaskList => BlockClass::List,
            Kind::ListItem | Kind::TaskListItem => BlockClass::ListItem,
            Kind::BlockQuote => BlockClass::Quote,
            Kind::CodeBlock => BlockClass::Code,
            Kind::Table => BlockClass::Table,
            Kind::Image => BlockClass::Media,
            // twig 2.8 folded `div`/`span`/`directive`/`element` into one
            // `container` kind, so a `:::note` panel and a promoted `<video>`
            // arrive here indistinguishable — telling them apart needs the
            // node's `origin`, and the incremental walk has only this kind.
            // `Directive` is the right answer for the case that motivates the
            // class (nothing else draws a tinted panel) and a harmless one for
            // the rest: `BlockClass` is descriptive, core never branches on it,
            // and the only frontend that reads a boundary spaces by
            // `below == Heading` alone. Anything that must be exact reads
            // [`container_is_directive`] off a real node.
            Kind::Container => BlockClass::Directive,
            Kind::ThematicBreak => BlockClass::Rule,
            Kind::Footnote => BlockClass::Footnote,
            _ => BlockClass::Other,
        }
    }
}

/// The name and attributes a leaf directive's placeholder row carries, so a
/// frontend that knows the host app's vocabulary can paint the real thing —
/// an embedded page for diaryx's `::embed{src=…}`, a generated table of
/// contents for a `::toc`, and the plain `⧉ name` label for one it doesn't
/// know. The peer of [`MediaMark`], and plain strings for the same reason: they
/// survive the row shuffling of [`BlockCache`] reuse and [`build_spliced`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectiveMark {
    /// The directive's type — `embed`, `toc`, `vis` — with no leading colons.
    /// Core is agnostic of what it means: the vocabulary is the host app's.
    pub name: String,
    /// Its `{…}` attributes as `(key, value)` pairs in source order. A bare
    /// attribute (`{public}`) has a `None` value, the way twig reports it.
    pub attrs: Vec<(String, Option<String>)>,
    /// The directive's `[label]` text, flattened from its inline children, or
    /// empty when it has none. Also what the placeholder label shows.
    pub label: String,
    /// How many visual rows this directive reserves — the label row plus blank
    /// filler rows below it, so a frontend painting something real has the
    /// vertical room. `1` is the bare placeholder, and the only value core
    /// produces today: unlike an image (whose height a terminal frontend
    /// measures and reports back), nothing has told core how tall an embed is.
    /// A pixel-laid-out GUI sets its own height regardless.
    pub rows: usize,
}

/// What a block-level media placeholder actually is, so a frontend knows which
/// widget to build over the reserved rows: a raster, a movie player, or a
/// transport with no picture at all. Core classifies and stops there — it opens
/// nothing, so this is a statement about the *markup*, not about a file it has
/// verified exists or can decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    /// A `![](…)` / `<img>` / `<picture>` — a still picture.
    Image,
    /// An HTML `<video>`. Markdown and Djot spell no video of their own, so this
    /// only ever arrives through `html_elements` promotion (or a `::video{…}`
    /// directive a host app maps itself, which core reports as a directive).
    Video,
    /// An HTML `<audio>` — a transport with no picture, so a frontend gives it a
    /// fixed control height rather than measuring an aspect ratio.
    Audio,
}

/// Which of the two caret homes a block media has — see
/// [`VisualMap::block_media_stop`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaStop {
    /// The stop in front of the picture. What is typed here belongs above it.
    Before,
    /// The stop just past it. What is typed here belongs below it.
    After,
}

impl MediaKind {
    /// The emoji a plain surface prefixes the placeholder label with — the
    /// `🖼`/`🎬`/`🔊` that makes the row read as *a thing* rather than as text.
    fn sigil(self) -> char {
        match self {
            MediaKind::Image => '🖼',
            MediaKind::Video => '🎬',
            MediaKind::Audio => '🔊',
        }
    }
}

/// The destination and label a block-level media placeholder row carries, so a
/// capable frontend can resolve and paint the real thing. Plain strings (no
/// source offsets), so they survive the row shuffling of [`BlockCache`] reuse
/// and [`build_spliced`] untouched — see [`VRow::media`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaMark {
    /// Whether this is a picture, a movie, or a sound — which widget the
    /// frontend builds over the reserved rows.
    pub kind: MediaKind,
    /// The media's link destination — a path, URL, or `data:` URI, verbatim from
    /// the AST. A frontend resolves a relative path against the document's
    /// directory itself; core holds no I/O.
    ///
    /// Empty is possible and legal for a `<video>`/`<audio>`, which may carry no
    /// `src` of its own and name its candidates in child `<source>`s instead —
    /// unlike an `<img>`, whose `src` *is* the picture. A frontend with an empty
    /// destination takes its URL from [`sources`](MediaMark::sources).
    pub destination: String,
    /// A `<picture>`'s theme/media alternatives, in document order, when this
    /// block image came from one; empty for a plain `![](…)` / bare `<img>`. Each
    /// is a `<source>`'s media query + candidate URL(s); a frontend that knows its
    /// theme picks the first whose media matches and falls back to [`destination`]
    /// (the `<img>`). Core keeps them verbatim and picks nothing — it has no theme.
    ///
    /// [`destination`]: MediaMark::destination
    pub sources: Vec<MediaSource>,
    /// The media's alt text (its rendered inline children, flattened), or empty
    /// when it has none. Also what the placeholder label shows. For a `<video>`/
    /// `<audio>` this is the element's own text content — the "your browser does
    /// not support…" fallback, which doubles as its accessible name.
    pub alt: String,
    /// A `<video poster="…">`'s still frame, verbatim, or empty when there is
    /// none (and always empty for an image or audio). It is an *image*
    /// destination, so a frontend already able to draw a picture can show it
    /// before the movie loads — or in place of one it can't play at all.
    pub poster: String,
    /// How many visual rows this media reserves — the placeholder label row plus
    /// the blank filler rows below it, so a frontend that paints a real raster has
    /// the vertical room to draw it. `1` is the bare placeholder (a frontend that
    /// can't draw pictures, or an image it couldn't resolve). A terminal frontend
    /// asks for as many rows as the fitted picture is tall; the pixel-laid-out GUI
    /// ignores this and sets its own row height, so it always leaves it `1`. The
    /// count comes from the frontend (via [`crate::Doc::set_media_rows`]) because
    /// core does no I/O and can't measure the image itself. See [`VRow::image`].
    pub rows: usize,
}

/// One `<source>` under a `<picture>`, `<video>`, or `<audio>`: a candidate URL
/// plus whichever of the two things HTML lets a `<source>` be chosen by — a
/// media query (`<picture>`) or a MIME type (`<video>`/`<audio>`). Verbatim from
/// the AST: core carries the alternatives and resolves none of them, having
/// neither a theme nor a codec list to judge them by.
///
/// The two spellings are normalised onto one field. `<picture>` writes
/// `srcset`, `<video>`/`<audio>` write `src`; both land in
/// [`srcset`](MediaSource::srcset), since a frontend wants the URL either way
/// and only `<picture>` ever uses the descriptor syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaSource {
    /// The `<source media="…">` query, verbatim (`"(prefers-color-scheme: dark)"`),
    /// or empty for a `<source>` with no `media` (an unconditional override, and
    /// the norm for `<video>`/`<audio>`, which pick by codec rather than theme).
    pub media: String,
    /// The candidate URL(s): a `<picture>`'s `srcset` verbatim — one URL, or a
    /// comma-separated candidate list with `1x`/`2x`/width descriptors — or a
    /// `<video>`/`<audio>` `<source>`'s plain `src`. A frontend takes the first
    /// URL token; the theme and codec cases both only ever need that.
    pub srcset: String,
    /// The `<source type="…">` MIME type (`"video/webm"`), verbatim, or empty
    /// when the `<source>` declares none. How a `<video>`/`<audio>` frontend
    /// picks a candidate it can actually decode; a `<picture>`'s sources
    /// normally leave it empty and are chosen by [`media`](MediaSource::media).
    pub mime: String,
}

/// The rendered document plus the offset⇄position mapping the caret rides on.
#[derive(Default)]
pub struct VisualMap {
    /// The document's **default monospace rendering** — one [`VRow`] of glyphs
    /// per visual line, tables spelled with box-drawing borders (`│ ─ ┌┬┐…`) and
    /// cells padded to whole character-cell columns. Any monospace surface can
    /// draw these verbatim, so a consumer gets a working view for free: the TUI
    /// paints them as-is, and a five-line plain-text dump would too.
    ///
    /// It's a *default*, not the only truth. A frontend with its own geometry —
    /// a proportional GUI — lays text out in its own units, and for a table
    /// skips the box-drawn rows named by [`TableInfo::rows_span`] and draws from
    /// the structural [`TableInfo`] instead. The box glyphs live here rather than
    /// in a frontend precisely because they *are* a renderable default: unlike a
    /// colour (a role each surface must map to its own palette — see
    /// [`crate::style`]), `┌─┐` is finished text that needs no interpretation.
    pub rows: Vec<VRow>,
    /// The first source offset that is actually rendered — the caret floor for
    /// the WYSIWYG view. Non-zero when a leading `metadata` block (YAML/TOML
    /// frontmatter) is skipped: the frontmatter is preserved in the source and
    /// editable in the source view, but hidden and unreachable here, so the
    /// caret and selection can't wander into it (and copy won't grab it).
    pub content_start: usize,
    /// Every offset the caret may rest at, ascending and deduplicated: each
    /// row's stop glyphs plus the row's own end (the "after the last character"
    /// spot every line needs). Decoration contributes nothing.
    ///
    /// Left/Right read this instead of walking the grid, because the grid isn't
    /// laid out in offset order: a table with wrapped cells puts column 1's
    /// second line *below* column 2's first, so "the next stop rightward" and
    /// "the next stop in the document" part ways. Following the document is what
    /// a caret means — and on every row that *is* in order the two agree anyway,
    /// so nothing else has to change.
    stops: Vec<usize>,
    /// Every table in the document, in order, described structurally rather than
    /// drawn — see [`TableInfo`] for why both exist.
    pub tables: Vec<TableInfo>,
    /// Every fenced/indented code block, in order, as the range of [`rows`] it
    /// occupies — a frontend draws one bordered, tinted box around each and
    /// scrolls it horizontally rather than wrapping. Derived from the per-row
    /// [`VRow::code`] flag once the rows are final (so it survives incremental
    /// row reuse), the same way [`collect_stops`] derives the stop table.
    ///
    /// [`rows`]: VisualMap::rows
    pub code_blocks: Vec<CodeBlockInfo>,
    /// Every block-level image in the document, in order — one per placeholder
    /// row a frontend replaces with a real picture. Derived from the per-row
    /// [`VRow::image`] mark once the rows are final (so it survives incremental
    /// row reuse), the same way [`code_blocks`](VisualMap::code_blocks) is
    /// derived from [`VRow::code`].
    pub media: Vec<MediaInfo>,
    /// Every **leaf** directive in the document, in order — one per placeholder
    /// row a frontend may replace with whatever the host app's vocabulary makes
    /// of it. Derived from the per-row [`VRow::leaf_directive`] mark once the
    /// rows are final, exactly as [`images`](VisualMap::images) is.
    pub directives: Vec<DirectiveInfo>,
}

impl VisualMap {
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    /// The width of `row` in display columns — the rightmost column its caret
    /// can occupy, and so what a goal column is clamped to on the way in.
    pub fn row_width(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, |r| r.width())
    }

    /// The screen `(row, col)` for a source offset — where to draw the caret:
    /// the *nearest* stop at or past `off`. Snaps a hidden offset (inside a
    /// delimiter) to the next visible glyph, and never resolves onto decoration
    /// (a table border, a cell's padding), which is drawn but holds no caret.
    ///
    /// "Nearest" rather than "the first one found" because a table's wrapped
    /// cells put rows slightly out of offset order: scanning top to bottom, the
    /// second line of column 1 comes *after* the first line of column 2 but
    /// holds smaller offsets. Where rows are in order the two rules agree.
    ///
    /// A soft wrap is the one place two rows want the same offset: the row above
    /// ends where the row below opens, the space the wrap ate being drawn on the
    /// row above and the offset past it being the row below's first character.
    /// It resolves *downstream*, to the row that character is on — the row
    /// above's last column is a phantom, a place the caret can be drawn but
    /// never sent, and resolving upstream into it is what pinned Down at the
    /// first wrap of a paragraph: it aimed at the row below's column 0, landed
    /// on the offset it already had, and read that back as the row above's end.
    pub fn pos_of_offset(&self, off: usize) -> (usize, usize) {
        let mut best: Option<(usize, usize, usize)> = None; // (src, row, col)
        for (r, row) in self.rows.iter().enumerate() {
            if row.decoration {
                continue;
            }
            // Offsets ascend *within* a row, so its first stop at or past `off`
            // is the best this row has to offer.
            let cand = row
                .glyphs
                .iter()
                .enumerate()
                .find(|(_, g)| g.stop && g.src >= off)
                .map(|(i, g)| (g.src, r, row.col_of_glyph(i)))
                .or_else(|| (row.end_src >= off).then_some((row.end_src, r, row.width())));
            if let Some(c) = cand {
                // `<=`, so a tie goes to the later row: the only offset two rows
                // both hold is a wrap boundary, and it belongs to the row below.
                if best.is_none_or(|b| c.0 <= b.0) {
                    best = Some(c);
                }
            }
            // A row's *first* stop never decreases from one row to the next —
            // true even across a table's wrapped cells, since a cell's lines run
            // downward. So once a row opens past the best found so far, no later
            // row can beat it and the scan stays proportional to `off`.
            if let (Some(b), Some(first)) = (best, row.glyphs.iter().find(|g| g.stop)) {
                if first.src > b.0 {
                    break;
                }
            }
        }
        match best {
            Some((_, r, c)) => (r, c),
            None => {
                let r = self.last_stop_row();
                (r, self.row_width(r))
            }
        }
    }

    /// The source offset of the task checkbox drawn at `(row, col)`, or `None`
    /// when that cell holds no box — the hit-test a frontend runs on a click
    /// before treating it as a tick rather than a caret placement.
    ///
    /// Only the box's own cells answer. Clicking an item's *text* places the
    /// caret like any other click, so the box is a target aimed at rather than
    /// something tripped over while editing — which is also why this is a
    /// separate question from [`offset_of_pos`](Self::offset_of_pos) instead of
    /// a flag on the offset it returns.
    pub fn task_box_at(&self, row: usize, col: usize) -> Option<usize> {
        let r = self.rows.get(row)?;
        self.task_box_at_glyph(row, r.glyph_at_col(col)?)
    }

    /// [`task_box_at`](Self::task_box_at) keyed by glyph index rather than
    /// display column — for a frontend that shapes its own rows (the GUI) and so
    /// resolves a click to a glyph before it ever has a column.
    pub fn task_box_at_glyph(&self, row: usize, glyph: usize) -> Option<usize> {
        let r = self.rows.get(row)?;
        r.task?;
        let g = r.glyphs.get(glyph)?;
        (g.style.role == Role::ListMarker).then_some(g.src)
    }

    /// The source offset for a screen `(row, col)` — where a click or a
    /// visual-space move lands the caret. Clicking decoration maps through its
    /// `src`, which points at the text it decorates, so a click on a border or
    /// on a cell's padding lands in that cell.
    ///
    /// The inverse of [`pos_of_offset`](Self::pos_of_offset), which it has to
    /// agree with: `col` is a display column, and the one it names may be the
    /// far cell of a wide glyph — [`VRow::glyph_at_col`] is where that lands.
    pub fn offset_of_pos(&self, row: usize, col: usize) -> usize {
        let Some(r) = self.rows.get(row) else {
            // A click or drag below the last row — a short document with empty
            // space under it, dragged into to extend a selection. Land on the
            // document's last caret stop (its end), not offset 0: jumping the
            // caret to the top is the wrong direction, and 0 isn't even a stop
            // when the document opens on hidden frontmatter or a `# ` marker, so
            // returning it would leave the caret where it draws in one place and
            // types in another (`move_to` would then clamp it onto the unhomeable
            // frontmatter floor). `None` only for a document with no stops at all
            // (empty), where the caret has nowhere to be but 0.
            return self.stops.last().copied().unwrap_or(0);
        };
        match r.glyph_at_col(col).and_then(|i| r.glyphs.get(i)) {
            // A glyph that holds no caret is clickable, but where it points
            // isn't always somewhere the caret can be: the blank gap between two
            // paragraphs stands at an offset that belongs to neither of them,
            // and the tail of a grapheme cluster stands inside a character.
            // Land on the nearest real stop instead of handing back an offset
            // that looks like the gap but types into the paragraph above.
            Some(g) if !g.stop => self.nearest_stop(g.src),
            Some(g) => g.src,
            // A row's end is a stop by construction — unless the row is
            // decoration, which contributes none.
            None if r.decoration => self.nearest_stop(r.end_src),
            None => r.end_src,
        }
    }

    /// Which of a block media's two caret homes `off` is, or `None` for every
    /// other offset in the document.
    ///
    /// [`block_media`](Builder::block_media) gives a block-level image, video, or
    /// audio exactly two stops — one in front of it and one just past it — and
    /// nothing inside the markup. Both are ordinary offsets to everything else in
    /// core, but they are the two places where inserting text would *dissolve the
    /// picture*: `![](p.png)` with anything typed against it is no longer a block
    /// image but a paragraph with an inline one, and the frontend that was
    /// painting a photo there paints a text run instead. A caller that is about to
    /// insert asks this so it can open a paragraph first — see
    /// [`Doc::insert`](crate::Doc::insert).
    ///
    /// An *inline* image reports `None`: it has no placeholder row and no stops of
    /// its own, and typing beside one is ordinary editing.
    ///
    /// Answers with the media's own source span as well, since a caller that has
    /// to keep the picture whole usually has to address it — [`Doc::backspace`]
    /// takes the picture out in one piece rather than nibbling a byte off its
    /// markup, which is the same dissolution from the other side.
    ///
    /// [`Doc::backspace`]: crate::Doc::backspace
    pub fn block_media_stop(&self, off: usize) -> Option<(MediaStop, Range<usize>)> {
        for m in &self.media {
            let Some(row) = self.rows.get(m.rows_span.start) else { continue };
            // Every glyph of the `🖼 alt` label maps to the media's start offset;
            // the row's end is past its markup. Read the start off the label
            // rather than the first glyph, which on a quoted or listed picture is
            // the block prefix and points at the gutter.
            let Some(start) = row
                .glyphs
                .iter()
                .find(|g| g.style.role == Role::Image)
                .map(|g| g.src)
            else {
                continue;
            };
            if off == start {
                return Some((MediaStop::Before, start..row.end_src));
            }
            if off == row.end_src {
                return Some((MediaStop::After, start..row.end_src));
            }
        }
        None
    }

    /// Snap `off` to the nearest caret stop — the funnel a frontend that
    /// hit-tests pixels straight to a source offset must run its result through.
    /// A click or drag can land in the blank gap a paragraph break is drawn with,
    /// or inside a hidden delimiter; both are offsets the caret can't rest at, so
    /// resting there would draw the caret in one place and type in another. This
    /// settles it on a real caret home instead. Idempotent on an offset that is
    /// already a stop — the `(row, col)` click path already snaps this way inside
    /// [`offset_of_pos`](Self::offset_of_pos), and this gives the pixel path the
    /// same guarantee. Returns `off` unchanged only for an empty document (no
    /// stops at all).
    pub fn snap_to_stop(&self, off: usize) -> usize {
        self.nearest_stop(off)
    }

    /// The caret stop nearest `off`, preferring the one before it when `off`
    /// falls exactly between two. Returns `off` unchanged if there are no stops
    /// at all (an empty document).
    fn nearest_stop(&self, off: usize) -> usize {
        let i = self.stops.partition_point(|&s| s < off);
        let after = self.stops.get(i).copied();
        let before = i.checked_sub(1).map(|j| self.stops[j]);
        match (before, after) {
            (Some(b), Some(a)) if off - b <= a - off => b,
            (_, Some(a)) => a,
            (Some(b), None) => b,
            (None, None) => off,
        }
    }

    /// Whether the caret can occupy `row` at all: decoration rows (a table's
    /// border rules) are stepped over by vertical motion.
    pub fn row_is_navigable(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| !r.decoration)
    }

    /// The first offset the caret can rest at on `row` — its first stop, or the
    /// row's own end when it holds no text (an empty paragraph). `None` for a
    /// decoration row, which holds no caret at all.
    ///
    /// Not `offset_of_pos(row, 0)`: column 0 of a quoted or listed row is the
    /// gutter, and a gutter's `src` points at the *block* it opens, so the stop
    /// nearest it is the one on the block's first row rather than on this one.
    /// Which is right for a click — the gutter decorates the whole block — and
    /// wrong for Home, whose whole question is where *this* row starts.
    pub fn row_start(&self, row: usize) -> Option<usize> {
        let r = self.rows.get(row).filter(|r| !r.decoration)?;
        Some(r.glyphs.iter().find(|g| g.stop).map_or(r.end_src, |g| g.src))
    }

    /// The last row the caret can rest on — the fallback when an offset is past
    /// everything rendered (a table's bottom border must not swallow the caret).
    fn last_stop_row(&self) -> usize {
        (0..self.rows.len())
            .rev()
            .find(|&r| self.row_is_navigable(r))
            .unwrap_or(0)
    }

    /// The nearest row above `row` the caret can occupy, skipping decoration.
    pub fn navigable_above(&self, row: usize) -> Option<usize> {
        (0..row.min(self.rows.len())).rev().find(|&r| self.row_is_navigable(r))
    }

    /// The nearest row below `row` the caret can occupy, skipping decoration.
    pub fn navigable_below(&self, row: usize) -> Option<usize> {
        ((row + 1)..self.rows.len()).find(|&r| self.row_is_navigable(r))
    }

    /// The caret stop just before `off` — one press of Left. `None` at the
    /// first stop in the document.
    ///
    /// Runs of decoration (a table border, a cell's alignment padding) are
    /// stepped over in a single press: they hold no stop, so they aren't in the
    /// table to land on.
    pub fn stop_before(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s < off);
        i.checked_sub(1).map(|i| self.stops[i])
    }

    /// The caret stop just after `off` — one press of Right. `None` at the last
    /// stop in the document.
    pub fn stop_after(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s <= off);
        self.stops.get(i).copied()
    }

    /// The first caret stop at or past `off` — where the caret at a hidden
    /// offset is *drawn*, and so where a rightward walk over the rendered text
    /// starts from.
    pub fn stop_at_or_after(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s < off);
        self.stops.get(i).copied()
    }

    /// The last caret stop at or before `off` — where a leftward walk starts
    /// from. Snapping the way the walk is headed, rather than always forward,
    /// is what keeps a leftward motion from ever moving the caret right.
    pub fn stop_at_or_before(&self, off: usize) -> Option<usize> {
        let i = self.stops.partition_point(|&s| s <= off);
        i.checked_sub(1).map(|i| self.stops[i])
    }

    /// Whether the caret may rest at `off` — the invariant every motion in this
    /// view has to leave standing.
    pub fn is_stop(&self, off: usize) -> bool {
        self.stops.binary_search(&off).is_ok()
    }

    /// The visible text a caret crosses walking rightward from `from` up to
    /// (but not including) `to` — `UITextInput.text(in:)`'s `[from, to)` in
    /// *this* view. A hidden inline-mark delimiter (`**`, `` ` ``, `_`, an
    /// escape backslash) never got a glyph in the first place — see
    /// [`push_text`]/[`synth`] — so it contributes nothing; what's left is
    /// exactly what's drawn on screen for that span.
    ///
    /// Built from the same stop glyphs [`stop_after`](Self::stop_after) steps
    /// across (every glyph with [`Glyph::stop`] set, i.e. one per grapheme
    /// cluster, decoration excluded) — **plus one inserted `'\n'` for every
    /// genuine block boundary strictly inside `[from, to)`**: a run of whole
    /// [`decoration`] rows sitting between two content rows — a paragraph
    /// gap, a table rule, an image's reserved filler rows — never an ordinary
    /// soft wrap, which puts no decoration *row* between the two halves of
    /// its one paragraph (only inline decoration glyphs, e.g. a table's `│`,
    /// live inside a single content row, and never split one).
    ///
    /// [`decoration`]: VRow::decoration
    ///
    /// Without that inserted break, two blocks abutting in this string were
    /// indistinguishable from one run of text: [`collect_stops`] gives a
    /// block boundary *zero* stops of its own (crossing one is a single,
    /// free hop — see `the_caret_skips_the_gap_between_two_paragraphs` in
    /// `doc.rs`'s tests, which pins that as intentional caret behaviour, a
    /// paragraph gap costing no extra Right presses, not a bug to fix here).
    /// So the last word of one paragraph and the first word of the next used
    /// to land directly adjacent with *nothing* between them in this string
    /// (`"...edb\n\nhello\n"` read back as `"edbhello"`), and `UITextInput`'s
    /// default word tokenizer then saw one unbroken run of letters and
    /// selected across the boundary — reported as double-tapping the last
    /// word on a line expanding the selection into the following
    /// paragraph(s).
    ///
    /// This means the once-strict equality with `distance_offset`/
    /// `step_offset` (`leaf-ffi`) no longer always holds: those intentionally
    /// keep costing a block boundary *zero* stops, while this text now
    /// spends one *character* on it that is never itself a stop. So the
    /// relationship is `visible_text(a, b).chars().count() >=
    /// distance_offset(a, b)`, equality holding whenever `(a, b)` spans no
    /// block boundary (the common case, and the only case the previous
    /// equality was ever tested against). It can only ever be *greater*,
    /// never less: every character this function omits relative to a plain
    /// stop count is a stop with no glyph of its own (a hidden delimiter, or
    /// a block's own trailing "end of row" stop), and every such omission at
    /// a block's end is exactly paired with the one inserted separator that
    /// follows it, so nothing this function returns is ever short of what a
    /// consumer walking stops one at a time would need. That inequality is
    /// still exactly what `UITextInput`'s tokenizer needs: it only ever reads
    /// this string to find a boundary and converts the character index it
    /// finds back to a position with `position(from:offset:)`, which walks
    /// stops — an inserted separator is never handed back as one, it only
    /// keeps two paragraphs' words apart for the tokenizer's letter-run scan.
    ///
    /// `from` is snapped to its nearest stop first, exactly as a caret asked
    /// to stand at a hidden offset is drawn at the next stop instead; `to` is
    /// left as given, so a stop landing exactly on it is still the walk's
    /// last step — the same asymmetry `distance_offset`'s own loop has.
    pub fn visible_text(&self, from: usize, to: usize) -> String {
        let from = self.nearest_stop(from);

        // Real content: every stop glyph in range, keyed by its own source
        // offset (`None` tags it as a genuine character, versus the
        // synthetic separators below).
        let mut items: Vec<(usize, Option<char>)> = self
            .rows
            .iter()
            .filter(|r| !r.decoration)
            .flat_map(|r| r.glyphs.iter())
            .filter(|g| g.stop && g.src >= from && g.src < to)
            .map(|g| (g.src, Some(g.ch)))
            .collect();

        // Every whole decoration row is a candidate block boundary; its
        // `end_src` is the gap offset itself (never a stop — see
        // `place_caret_snaps_out_of_the_blank_gap_between_paragraphs` in
        // `doc.rs`) — a source offset like any glyph's, so it merges into the
        // same ordering. `None` marks it a synthetic separator rather than a
        // real character, tagged distinctly so a query landing exactly on the
        // gap offset still opens with its break even with no glyph on either
        // side to anchor it to (a range spanning nothing but a bare gap).
        let mut boundaries: Vec<usize> = self
            .rows
            .iter()
            .filter(|r| r.decoration)
            .map(|r| r.end_src)
            .filter(|&src| src >= from && src < to)
            .collect();
        boundaries.sort_unstable();
        boundaries.dedup();
        items.extend(boundaries.into_iter().map(|src| (src, None)));

        // Row order matches source order except across a table's wrapped
        // cells (see `pos_of_offset`), so sort rather than trust it here too.
        // A boundary can't share an offset with a glyph (it's the undrawn gap
        // between two blocks' real content), so tie-breaking never arises.
        items.sort_by_key(|&(src, _)| src);
        items.into_iter().map(|(_, ch)| ch.unwrap_or('\n')).collect()
    }
}

/// Collect the caret stops of a laid-out grid: every stop glyph's offset plus
/// every row's end, ascending and deduplicated. Duplicates are the norm rather
/// than the exception — a wrapped line's end is the same offset as the next
/// line's first glyph — and collapsing them is what makes one press of Left or
/// Right cross exactly one stop.
fn collect_stops(rows: &[VRow]) -> Vec<usize> {
    let mut stops: Vec<usize> = rows
        .iter()
        .filter(|r| !r.decoration)
        .flat_map(|r| {
            r.glyphs
                .iter()
                .filter(|g| g.stop)
                .map(|g| g.src)
                .chain(std::iter::once(r.end_src))
        })
        .collect();
    stops.sort_unstable();
    stops.dedup();
    stops
}

/// Group the rows tagged [`VRow::code`] into one [`CodeBlockInfo`] per maximal
/// run — the block-level view a frontend needs to box and scroll each code
/// block. Two code blocks are always parted by the blank separator row a block
/// boundary is spelled with (never itself a code row), so a contiguous run is
/// exactly one block. Derived from the final rows rather than tracked through
/// the builder so it comes out right no matter how [`build_cached`] and
/// [`build_spliced`] shuffle rows around.
fn code_block_spans(rows: &[VRow]) -> Vec<CodeBlockInfo> {
    let mut blocks = Vec::new();
    let mut start: Option<usize> = None;
    for (i, row) in rows.iter().enumerate() {
        match (row.code, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                blocks.push(CodeBlockInfo { rows_span: s..i, lang: rows[s].code_lang.clone() });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        blocks.push(CodeBlockInfo { rows_span: s..rows.len(), lang: rows[s].code_lang.clone() });
    }
    blocks
}

/// Collect one [`MediaInfo`] per row carrying an [`VRow::image`] mark — the
/// block-level view a frontend needs to replace each placeholder row with a real
/// picture. The mark rides the block's *first* row and names how many rows the
/// image reserves ([`MediaMark::rows`]); the rows below it are blank
/// [`decoration`](VRow::decoration) fillers that hold the vertical space and no
/// caret. So the span runs from the marked row across those fillers. Derived from
/// the final rows rather than tracked through the builder so it survives however
/// [`build_cached`] and [`build_spliced`] shuffle rows around.
/// The value of `node`'s `key` attribute, if it carries one *with* a value. A
/// bare attribute (`controls`, `muted`) has a `None` value and so reads as
/// absent here — a caller wanting presence-not-value tests the list directly.
/// Shared by the media element and `<source>` readers.
fn attr_of(node: &FlatNode, key: &str) -> Option<String> {
    node.attrs.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.clone())
}

fn media_spans(rows: &[VRow]) -> Vec<MediaInfo> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, row)| {
            row.media.as_ref().map(|m| MediaInfo {
                rows_span: i..i + m.rows.max(1),
                kind: m.kind,
                destination: m.destination.clone(),
                sources: m.sources.clone(),
                alt: m.alt.clone(),
                poster: m.poster.clone(),
            })
        })
        .collect()
}

/// Collect one [`DirectiveInfo`] per row carrying a [`VRow::leaf_directive`]
/// mark — the block-level view a frontend needs to replace each placeholder row
/// with whatever the directive means to it. The peer of [`media_spans`], derived
/// from the final rows for the same reason: it survives however [`build_cached`]
/// and [`build_spliced`] shuffle rows around.
fn directive_spans(rows: &[VRow]) -> Vec<DirectiveInfo> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, row)| {
            row.leaf_directive.as_ref().map(|m| DirectiveInfo {
                rows_span: i..i + m.rows.max(1),
                name: m.name.clone(),
                attrs: m.attrs.clone(),
                label: m.label.clone(),
            })
        })
        .collect()
}

/// The source range of a fenced code block's info string — everything on the
/// opening line past the fence (`` ```rust `` → the `rust`). `block_start` is the
/// code block node's `span.start`. `None` for an indented code block, which
/// opens with no fence to carry one. The range is empty for a fence written
/// bare (`` ``` `` alone), which is exactly where a language would be inserted.
///
/// Shared by the WYSIWYG builder (to label the box) and [`crate::Doc`] (to edit
/// the label through a prompt), so the two agree on where the language lives.
pub fn code_info_span(source: &str, block_start: usize) -> Option<Range<usize>> {
    let rest = source.get(block_start..)?;
    let line_len = rest.find('\n').unwrap_or(rest.len());
    let line = &rest[..line_len];
    // A fence may be indented up to three spaces; past that it opens with a run
    // of the same fence character.
    let indent = line.len() - line.trim_start().len();
    if indent > 3 {
        return None;
    }
    let fence = line[indent..].chars().next()?;
    if fence != '`' && fence != '~' {
        return None; // an indented block, not a fenced one
    }
    let fence_len = line[indent..].chars().take_while(|&c| c == fence).count();
    let info_start = block_start + indent + fence_len;
    Some(info_start..block_start + line_len)
}

/// A fenced code block's language for display: its info string, trimmed, or
/// `None` when there's no fence or the fence carries no language. The trimmed
/// text is what a frontend labels the box with; [`code_info_span`] is what an
/// edit replaces.
pub fn code_language(source: &str, block_start: usize) -> Option<String> {
    let span = code_info_span(source, block_start)?;
    let text = source.get(span)?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A horizontal rule's dash count when the map isn't wrapping to a column grid
/// (the GUI, which wraps at pixel width): a fixed, sane width the frontend can
/// paint or re-wrap, instead of a runaway count from an unbounded wrap width.
const UNWRAPPED_RULE_WIDTH: usize = 40;

/// Render the document to a [`VisualMap`]. `wrap` is the column budget for
/// word-wrapping (`Some` for the monospace TUI), or `None` to emit one row per
/// block — the GUI does its own proportional pixel wrapping over these rows.
/// Text and offsets come from the AST (`str` nodes carry the verbatim source
/// slice and an exact span), so the original source string isn't needed here.
pub fn build(
    nodes: &[FlatNode],
    source: &str,
    wrap: Option<usize>,
    preserve_soft: bool,
    media_rows: &HashMap<String, usize>,
    reveal: Option<Range<usize>>,
) -> VisualMap {
    let Some(doc) = nodes.iter().position(|n| n.kind == Kind::Doc) else {
        return VisualMap::default();
    };
    let top = top_level(nodes, doc);
    let mut b = Builder {
        nodes,
        source,
        wrap: wrap.map(|w| w.max(8)),
        rows: Vec::new(),
        tables: Vec::new(),
        last_off: 0,
        media_rows,
        break_glyph: Cell::new(' '),
        preserve_soft,
        reveal: reveal.clone(),
    };
    b.top_blocks(&top);
    b.emit_trailing_blank_lines(
        top.last().map_or(BlockClass::Paragraph, |&i| BlockClass::from_node_kind(&nodes[i].kind)),
    );
    let content_start = top.first().map_or(0, |&i| nodes[i].span.start);
    let stops = collect_stops(&b.rows);
    let code_blocks = code_block_spans(&b.rows);
    let media = media_spans(&b.rows);
    let directives = directive_spans(&b.rows);
    VisualMap {
        rows: b.rows,
        content_start,
        stops,
        tables: b.tables,
        code_blocks,
        media,
        directives,
    }
}

/// Like [`build`], but reuses a persistent [`BlockCache`] so an edit re-renders
/// only the top-level blocks whose source bytes changed *and* marshals only
/// those blocks from twig instead of the whole arena.
///
/// `top` is the document's top-level blocks — twig's `child_spans` of the doc
/// root: `(node_id, kind, span)` for each, in order. `fetch_subtree(node_id)`
/// marshals one block's subtree (local-indexed, root at 0) and is called *only*
/// for a block that missed the cache, i.e. one that actually changed. So a
/// keystroke marshals one small subtree, not ~20k nodes. The result is
/// byte-for-byte identical to [`build`] on the same document (the
/// `build_cached_matches_build` test pins this); [`build`] stays the cache-free,
/// whole-arena reference. This is the entry point [`crate::Doc`] uses.
pub fn build_cached(
    top: &[QueryMatch],
    source: &str,
    wrap: Option<usize>,
    preserve_soft: bool,
    media_rows: &HashMap<String, usize>,
    reveal: Option<Range<usize>>,
    cache: &mut BlockCache,
    mut fetch_subtree: impl FnMut(u32) -> Vec<FlatNode>,
) -> VisualMap {
    let wrap = wrap.map(|w| w.max(8));

    // Wrapping is a function of the width, so a width change makes every cached
    // row's wrap wrong: start the cache over.
    if cache.wrap != Some(wrap) {
        cache.entries.clear();
        cache.wrap = Some(wrap);
    }
    cache.generation = cache.generation.wrapping_add(1);

    // Frontmatter (a leading `metadata` block) is document metadata, not prose:
    // hidden in the rich view exactly as [`Builder::blocks`] skips it.
    let blocks: Vec<&QueryMatch> = top.iter().filter(|m| m.kind != Kind::Metadata).collect();

    // The outer builder only accumulates rows/tables and spells block boundaries
    // — both a function of the source and `last_off`, never of a node array — so
    // it carries an empty `nodes`. Each changed block is rendered by a *fresh*
    // builder over that block's subtree.
    let mut b = Builder {
        nodes: &[],
        source,
        wrap,
        rows: Vec::new(),
        tables: Vec::new(),
        last_off: 0,
        media_rows,
        break_glyph: Cell::new(' '),
        preserve_soft,
        reveal: reveal.clone(),
    };

    // Record the per-block row decomposition as we go, so a later
    // [`build_spliced`] can patch one block without rebuilding the map.
    let mut layout_blocks: Vec<BlockLayout> = Vec::with_capacity(blocks.len());
    let mut all_shift_safe = true;
    for (i, block) in blocks.iter().enumerate() {
        let start = block.span.start;
        let before_sep = b.rows.len();
        if i > 0 {
            // This walker has no node arena at all (see the `nodes: &[]` above),
            // but a top-level query match carries its kind — the same string
            // `BlockClass::from_node_kind` classifies for the whole-arena walk, so
            // the incremental and full builds label a boundary identically.
            b.emit_separators_before(
                start,
                &[],
                true,
                Boundary {
                    above: BlockClass::from_node_kind(&blocks[i - 1].kind),
                    below: BlockClass::from_node_kind(&block.kind),
                },
            );
        }
        let sep_rows = b.rows.len() - before_sep;
        let after_sep = b.rows.len();
        let bytes = block_bytes(source, &block.span);
        let hash = block_hash(bytes);
        // How this block meets the reveal line, if at all — part of its cache
        // key, since the same bytes render differently on the caret's line.
        let rkey = reveal_key(&reveal, &block.span);

        // Hit: clone the block's rows shifted to its current offset and restore
        // the (shifted) `last_off` so the next separator lands right — no marshal.
        // Only shift-safe blocks are ever cached, so a hit is safe by construction.
        if let Some(hit) = cache.reuse(hash, bytes, &rkey) {
            let delta = start as isize - hit.built_start as isize;
            for row in &hit.rows {
                b.rows.push(shift_row(row, delta));
            }
            b.last_off = (hit.last_off as isize + delta) as usize;
        } else {
            // Miss: marshal just this block's subtree and render it. A subtree is
            // self-contained with local ids (root at 0) and absolute spans, so a
            // fresh builder over it produces the same rows the whole-arena path
            // would. An empty subtree (twig couldn't hand it back) renders nothing.
            let subtree = fetch_subtree(block.node_id);
            if !subtree.is_empty() {
                let mut sub = Builder {
                    nodes: &subtree,
                    source,
                    wrap,
                    rows: Vec::new(),
                    tables: Vec::new(),
                    last_off: 0,
                    media_rows,
                    break_glyph: Cell::new(' '),
                    preserve_soft,
                    reveal: reveal.clone(),
                };
                sub.block(0, &[], &[]);
                let last_off = sub.last_off;
                // Cache only a block that is table-free AND renders inside its own
                // span: those two are the conditions for reuse-by-shift to be
                // correct. A block failing either is re-rendered every build (a
                // fresh render always matches a fresh whole-document build).
                if sub.tables.is_empty() {
                    if rows_within(&sub.rows, &block.span) {
                        cache.store(hash, bytes, start, sub.rows.clone(), last_off, rkey);
                    }
                    b.rows.extend(sub.rows);
                } else {
                    // A table block is never cached; rebase its row-index
                    // bookkeeping onto the combined row vector and append.
                    let base = b.rows.len();
                    for t in &mut sub.tables {
                        t.rows_span = (t.rows_span.start + base)..(t.rows_span.end + base);
                    }
                    b.rows.extend(sub.rows);
                    b.tables.extend(sub.tables);
                }
                b.last_off = last_off;
            }
        }
        let content_rows = b.rows.len() - after_sep;
        all_shift_safe &= rows_within(&b.rows[after_sep..], &block.span);
        layout_blocks.push(BlockLayout {
            span: block.span.clone(),
            kind: block.kind.clone(),
            sep_rows,
            content_rows,
        });
    }

    let before_trailing = b.rows.len();
    b.emit_trailing_blank_lines(
        blocks.last().map_or(BlockClass::Paragraph, |m| BlockClass::from_node_kind(&m.kind)),
    );
    let trailing_rows = b.rows.len() - before_trailing;

    // Evict every entry no block reused this build, so the cache tracks the
    // current document instead of growing without bound over a session.
    let g = cache.generation;
    cache.entries.retain(|_, bucket| {
        bucket.retain(|e| e.generation == g);
        !bucket.is_empty()
    });

    cache.layout = Layout {
        blocks: layout_blocks,
        trailing_rows,
        built_len: source.len(),
        has_tables: !b.tables.is_empty(),
        all_shift_safe,
        reveal: reveal.clone(),
    };

    // The first rendered offset is the first non-metadata block's start (0 when
    // the document is empty or all frontmatter) — the analogue of
    // [`first_content_offset`] for the top-level list.
    let content_start = blocks.first().map_or(0, |m| m.span.start);
    let stops = collect_stops(&b.rows);
    let code_blocks = code_block_spans(&b.rows);
    let media = media_spans(&b.rows);
    let directives = directive_spans(&b.rows);
    VisualMap {
        rows: b.rows,
        content_start,
        stops,
        tables: b.tables,
        code_blocks,
        media,
        directives,
    }
}

/// The fast path for a single-block edit: patch the previous [`VisualMap`] in
/// place rather than reassembling it. Returns `Some(new_map)` when it applies,
/// or `None` to tell the caller to fall back to [`build_cached`] (always
/// correct). Consumes `prev` either way — on `None` the caller rebuilds from
/// scratch and doesn't need it.
///
/// It applies only when `dirty` (twig's dirty byte range) falls inside exactly
/// one top-level block AND the block structure around it is unchanged — verified
/// by matching the new `top` list against the previous [`Layout`] block for
/// block: kinds unchanged, spans before the edit identical, spans after it
/// shifted by the byte delta, count unchanged. Any deviation — a block split or
/// merged, a fence opened to swallow later blocks, a table anywhere, a
/// multi-block edit — fails the match and returns `None`. That check is what
/// makes the byte-range trustworthy: twig's dirty range is exact about *bytes*
/// but silent about *reparse*, and the structural match catches the reparse
/// effects it can't see.
///
/// When it applies, the unchanged prefix rows move verbatim, the suffix rows
/// shift by the delta *in place* (integer adds, no glyph copy), and only the one
/// dirty block is re-marshalled and re-rendered; stops splice the same way by
/// offset. So the cost is O(rows after the edit), and nothing before the edit is
/// touched. The hash-keyed entry cache is left alone — a later [`build_cached`]
/// will miss on the changed block, re-render it, and evict the stale entry, so
/// chained splices neither corrupt nor grow it.
pub fn build_spliced(
    prev: VisualMap,
    source: &str,
    wrap: Option<usize>,
    preserve_soft: bool,
    top: &[QueryMatch],
    dirty: Range<usize>,
    media_rows: &HashMap<String, usize>,
    reveal: Option<Range<usize>>,
    cache: &mut BlockCache,
    mut fetch_subtree: impl FnMut(u32) -> Vec<FlatNode>,
) -> Option<VisualMap> {
    let wrap = wrap.map(|w| w.max(8));
    // A width change invalidates every cached row — a full rebuild's job.
    if cache.wrap != Some(wrap) {
        return None;
    }
    // So does a moved reveal line, and for the same reason: this path reuses
    // every row outside the dirty block, and those rows encode which line was
    // showing its raw markup when they were built. Typing almost always moves
    // the caret, so under `MarkupMode::Full` this bails to `build_cached` on
    // most keystrokes — still block-cached, so only the edited block and the
    // revealed one actually re-render.
    if cache.layout.reveal != reveal {
        return None;
    }
    // Take the previous layout; on any bail below the caller rebuilds it (and the
    // map) via `build_cached`, so leaving it empty is fine. A table or a block
    // that renders outside its span (a degenerate inline span) makes shifting
    // unsound, so those force the full-rebuild path.
    let prev_layout = std::mem::take(&mut cache.layout);
    if prev_layout.built_len == 0 || prev_layout.has_tables || !prev_layout.all_shift_safe {
        return None;
    }

    let blocks: Vec<&QueryMatch> = top.iter().filter(|m| m.kind != Kind::Metadata).collect();
    if blocks.is_empty() || blocks.len() != prev_layout.blocks.len() {
        return None;
    }
    let delta = source.len() as isize - prev_layout.built_len as isize;

    // The single block whose NEW span contains the whole dirty range. A dirty
    // range straddling a block boundary (or a separator) finds none → bail.
    let k = blocks
        .iter()
        .position(|m| m.span.start <= dirty.start && dirty.end <= m.span.end)?;

    // Structural match: every OTHER block is unchanged — same kind throughout,
    // span identical before the edit and shifted by `delta` after it. A mismatch
    // means the reparse reshaped the block structure, which only a full rebuild
    // renders correctly.
    for (i, (m, pl)) in blocks.iter().zip(&prev_layout.blocks).enumerate() {
        if m.kind != pl.kind {
            return None;
        }
        if i == k {
            continue;
        }
        let want = if i < k {
            pl.span.clone()
        } else {
            (pl.span.start as isize + delta) as usize..(pl.span.end as isize + delta) as usize
        };
        if m.span != want {
            return None;
        }
    }
    // The dirty block itself: start unchanged (the edit is inside it, past its
    // start), end moved by exactly the delta.
    let pk_start = prev_layout.blocks[k].span.start;
    let pk_end = prev_layout.blocks[k].span.end;
    let pk_sep = prev_layout.blocks[k].sep_rows;
    let pk_content = prev_layout.blocks[k].content_rows;
    if blocks[k].span.start != pk_start
        || blocks[k].span.end != (pk_end as isize + delta) as usize
    {
        return None;
    }

    // Re-render the dirty block from its subtree. A table makes the splice
    // bookkeeping unsafe, so bail if one appears.
    let subtree = fetch_subtree(blocks[k].node_id);
    if subtree.is_empty() {
        return None;
    }
    let mut sub = Builder {
        nodes: &subtree,
        source,
        wrap,
        rows: Vec::new(),
        tables: Vec::new(),
        last_off: 0,
        media_rows,
        break_glyph: Cell::new(' '),
        preserve_soft,
        reveal: reveal.clone(),
    };
    sub.block(0, &[], &[]);
    // A table, or content that renders outside the block's span (a degenerate
    // inline span), makes the shift bookkeeping unsound — fall back.
    if !sub.tables.is_empty() || !rows_within(&sub.rows, &blocks[k].span) {
        return None;
    }
    let new_content = sub.rows;
    let new_content_len = new_content.len();
    let new_stops = collect_stops(&new_content);

    // Row span of the dirty block's CONTENT. Its leading separator stays in the
    // prefix: the gap before block k is unchanged, since k's start didn't move.
    let content_start_row: usize = prev_layout.blocks[..k]
        .iter()
        .map(|pl| pl.sep_rows + pl.content_rows)
        .sum::<usize>()
        + pk_sep;
    let content_end_row = content_start_row + pk_content;

    // Splice rows: [prefix | new content | suffix + delta]. The prefix moves
    // untouched; the suffix shifts in place — integer adds, no glyph copy.
    let mut rows = prev.rows;
    let mut suffix = rows.split_off(content_end_row);
    rows.truncate(content_start_row);
    for row in &mut suffix {
        shift_row_in_place(row, delta);
    }
    rows.reserve(new_content_len + suffix.len());
    rows.extend(new_content);
    rows.extend(suffix);

    // Splice stops by offset. The old dirty block covered `[pk_start, pk_end]`:
    // prefix stops fall below it, suffix stops above it (shift by delta), the new
    // content supplies the middle. The three ranges stay disjoint and ascending,
    // so the result needs no re-sort.
    let p1 = prev.stops.partition_point(|&s| s < pk_start);
    let p2 = prev.stops.partition_point(|&s| s <= pk_end);
    let mut stops = Vec::with_capacity(p1 + new_stops.len() + (prev.stops.len() - p2));
    stops.extend_from_slice(&prev.stops[..p1]);
    stops.extend(new_stops);
    for &s in &prev.stops[p2..] {
        stops.push((s as isize + delta) as usize);
    }

    // Record the patched layout for the next splice: spans move to the new
    // coordinates, and the dirty block takes its new content-row count.
    let mut new_blocks = prev_layout.blocks;
    for (pl, m) in new_blocks.iter_mut().zip(&blocks) {
        pl.span = m.span.clone();
    }
    new_blocks[k].content_rows = new_content_len;
    cache.layout = Layout {
        blocks: new_blocks,
        trailing_rows: prev_layout.trailing_rows,
        built_len: source.len(),
        has_tables: false,
        // Every prefix/suffix block was shift-safe last build (we bailed
        // otherwise) and the re-rendered block was just checked, so the patched
        // document is still entirely shift-safe.
        all_shift_safe: true,
        reveal,
    };

    let code_blocks = code_block_spans(&rows);
    let media = media_spans(&rows);
    let directives = directive_spans(&rows);
    Some(VisualMap {
        rows,
        content_start: blocks[0].span.start,
        stops,
        tables: Vec::new(),
        code_blocks,
        media,
        directives,
    })
}

/// A persistent, content-keyed cache of the rows each top-level block renders
/// to — the [`VisualMap`] analogue of the GUI's ShapedLine cache, one level
/// down. Held by a [`crate::Doc`] and threaded into [`build_cached`], it is what
/// makes a rebuild after a keystroke cost "re-render the edited block + shift
/// the rest" instead of re-rendering the whole document.
///
/// A top-level block's rows are a pure function of its source bytes and the wrap
/// width, so an unchanged block's rows are cloned and their source offsets
/// shifted by the edit's byte delta rather than rebuilt glyph by glyph. Two
/// things make that purity hold: at the top level the render prefix is always
/// empty (nesting prefixes — a quote gutter, a list indent — exist only *inside*
/// a top-level block, within its cached unit), and a block's output never reads
/// the incoming `last_off` (it writes `last_off` from its own content before any
/// nested separator reads it). So the only thing that differs between two
/// positions of an unchanged block is a uniform offset shift. Keyed by a fast
/// hash of the block's bytes with the bytes kept for a verify-on-hit — exactly
/// the shape cache's weak-hash-then-compare, so a collision costs a re-render,
/// never a wrong row.
///
/// Tables are never cached (a block that emits any table row is always rebuilt):
/// their rows are cross-referenced from the map's `tables` side-table by row
/// index, which a blind offset-shift wouldn't fix up, and they are rare enough
/// that the simplicity beats the reuse.
#[derive(Default)]
pub struct BlockCache {
    /// The wrap width every entry was built at; a change invalidates all of
    /// them. `None` before the first build (distinct from `Some(None)`, the
    /// unwrapped GUI width).
    wrap: Option<Option<usize>>,
    /// Bumped once per [`build_cached`]. An entry reused or inserted this build
    /// carries the current value; stale entries are dropped at the end of it.
    generation: u64,
    /// `hash(bytes)` → the block(s) sharing that hash — a bucket because
    /// distinct blocks can collide, while two *identical* blocks share one entry
    /// (free dedup).
    entries: HashMap<u64, Vec<CachedBlock>>,
    /// The row/stop decomposition of the last build, which [`build_spliced`]
    /// patches in place for a single-block edit. Kept in step with whatever
    /// [`VisualMap`] was last produced; empty before the first build.
    layout: Layout,
}

/// How the last build's [`VisualMap`] decomposes into top-level blocks — the
/// bookkeeping [`build_spliced`] needs to splice one block's rows and stops
/// without rebuilding the whole map. Every field describes the *previous* build,
/// in that build's coordinates.
#[derive(Default)]
struct Layout {
    /// One entry per rendered (metadata-filtered) top-level block, in order.
    blocks: Vec<BlockLayout>,
    /// Trailing blank rows past the last block (from `emit_trailing_blank_lines`).
    trailing_rows: usize,
    /// The source length this layout was built at — the reference for the edit's
    /// byte delta.
    built_len: usize,
    /// Whether the last build drew any table. A table's cross-referenced row
    /// indices don't survive a blind splice, so their presence makes
    /// [`build_spliced`] bail to a full rebuild.
    has_tables: bool,
    /// Whether every block rendered strictly inside its own span (see
    /// [`rows_within`]). A block that doesn't — a malformed Markdown inline node
    /// that twig leaves with a degenerate `0..0` span renders at a fixed offset
    /// outside its block — can't be shifted correctly, so its presence makes
    /// [`build_spliced`] bail to a full rebuild.
    all_shift_safe: bool,
    /// The reveal line this layout was built under (see [`Builder::reveal`]).
    /// A splice reuses every row it isn't re-rendering, so a reveal line that
    /// has moved would leave the old line still showing its delimiters and the
    /// new one still hiding them — [`build_spliced`] bails when this changes.
    reveal: Option<Range<usize>>,
}

/// One top-level block's contribution to the last build: its span and kind (for
/// the structural match that proves only one block changed) and how many
/// separator and content rows it emitted (to locate its slice of the row
/// vector).
struct BlockLayout {
    span: Range<usize>,
    kind: Kind,
    sep_rows: usize,
    content_rows: usize,
}

/// One cached block: the rows it rendered to, plus what a reuse at a new
/// position needs to shift them. Offsets are stored absolute (as built) and
/// shifted by `new_start - built_start` on reuse.
struct CachedBlock {
    /// The block's exact source bytes, compared on a hash hit so a collision
    /// can never hand back another block's rows.
    bytes: Box<[u8]>,
    /// The offset the rows were built at (the block's `span.start`).
    built_start: usize,
    /// The block's rows, offsets absolute as built.
    rows: Vec<VRow>,
    /// `last_off` after this block was emitted, absolute as built — restored
    /// (shifted) on reuse so the following separator lands correctly.
    last_off: usize,
    /// Where the reveal line fell *within this block* when the rows were built,
    /// as a block-relative byte range — see [`reveal_key`]. Compared alongside
    /// `bytes` on a hit, because identical source renders to different rows
    /// depending on whether the caret's line is inside it: the same `*em*`
    /// shows its asterisks on the revealed line and hides them everywhere else.
    ///
    /// Block-relative rather than absolute so an unaffected block still hits
    /// after an edit shifts it, and `None` for the overwhelmingly common
    /// no-reveal case — which is why an entry stored under `MarkupMode::None`
    /// keeps hitting for every block that isn't the caret's.
    reveal: Option<Range<usize>>,
    /// The build that last reused or inserted this entry (see `generation`).
    generation: u64,
}

/// Where `reveal` falls inside a block, in block-relative bytes — the extra key
/// a cached block is stored and matched under.
///
/// `None` when the block doesn't meet the reveal line at all, which is every
/// block on every build in the two hidden modes, and all but one of them under
/// [`crate::MarkupMode::Full`]. So the cache keeps its hit rate as the caret
/// moves: only the line the caret leaves and the line it arrives at re-render.
fn reveal_key(reveal: &Option<Range<usize>>, span: &Range<usize>) -> Option<Range<usize>> {
    let r = reveal.as_ref()?;
    // The same generous intersection test `Builder::revealed` uses, so a block
    // is keyed as revealed exactly when its glyphs will be built that way.
    (span.start <= r.end && r.start <= span.end).then(|| {
        let start = r.start.max(span.start) - span.start;
        let end = r.end.min(span.end) - span.start;
        start..end
    })
}

impl BlockCache {
    /// Look up a block by hash, verify its bytes and reveal key, and on a hit
    /// stamp it used this build and hand back a borrow to shift-and-clone from.
    /// `None` on a miss (unknown hash, a collision whose bytes differ, or the
    /// same bytes built under a different reveal).
    fn reuse(&mut self, hash: u64, bytes: &[u8], reveal: &Option<Range<usize>>) -> Option<&CachedBlock> {
        let g = self.generation;
        let bucket = self.entries.get_mut(&hash)?;
        let e = bucket.iter_mut().find(|e| &*e.bytes == bytes && &e.reveal == reveal)?;
        e.generation = g;
        Some(&*e)
    }

    /// Cache the rows a freshly-rendered block produced (or refresh an existing
    /// entry for the same bytes and reveal — an identical block elsewhere, or a
    /// re-render).
    fn store(
        &mut self,
        hash: u64,
        bytes: &[u8],
        built_start: usize,
        rows: Vec<VRow>,
        last_off: usize,
        reveal: Option<Range<usize>>,
    ) {
        let g = self.generation;
        let bucket = self.entries.entry(hash).or_default();
        if let Some(e) = bucket.iter_mut().find(|e| &*e.bytes == bytes && e.reveal == reveal) {
            e.built_start = built_start;
            e.rows = rows;
            e.last_off = last_off;
            e.generation = g;
        } else {
            bucket.push(CachedBlock {
                bytes: bytes.into(),
                built_start,
                rows,
                last_off,
                reveal,
                generation: g,
            });
        }
    }
}

/// The source bytes a top-level block covers — the block cache's key material.
///
/// Clamped to the source rather than sliced by the span as twig gives it,
/// because that span can end *past* the last byte: the final block of a document
/// with no trailing newline is closed on the virtual newline the parser supplies
/// at EOF, so its `span.end` is `source.len() + 1`. Slicing by such a range
/// yields `None`, and the obvious `unwrap_or(&[])` reads that as *this block has
/// no bytes* — the wrong answer twice over.
///
/// Two blocks whose spans both overrun then key alike, and the second is served
/// the first one's rows. That is not hypothetical: a footnote definition is a
/// root beside `doc` merged back into the top level by [`top_blocks`], while the
/// `section` above it spans the definition's bytes too, so both end at EOF —
/// and a document ending in `[^note]: …` renders that definition as a second
/// copy of the heading. Even alone, a block that keeps hashing empty as the user
/// types in it is served the stale rows built before the edit.
///
/// Clamping hands back the bytes the block really covers, which tells both cases
/// apart, and costs nothing for a span that was in range to begin with.
fn block_bytes<'a>(source: &'a str, span: &Range<usize>) -> &'a [u8] {
    let bytes = source.as_bytes();
    let start = span.start.min(bytes.len());
    &bytes[start..span.end.clamp(start, bytes.len())]
}

/// A fast, allocation-free content hash (FNV-1a) for a block's bytes. Weak by
/// design — the bytes are compared on a hit — so its only job is to spread
/// blocks across buckets cheaply. SipHash over every block's bytes on every
/// keystroke would cost more than it saves, the same lesson the shape cache
/// learned when it stopped hashing through the standard hasher.
fn block_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in bytes {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Clone a cached row with every source offset advanced by `delta` — the whole
/// cost of reusing an unchanged block: integer adds where a rebuild would
/// re-shape every glyph.
fn shift_row(row: &VRow, delta: isize) -> VRow {
    let shift = |off: usize| (off as isize + delta) as usize;
    VRow {
        glyphs: row
            .glyphs
            .iter()
            .map(|g| Glyph {
                ch: g.ch,
                style: g.style,
                src: shift(g.src),
                stop: g.stop,
            })
            .collect(),
        end_src: shift(row.end_src),
        decoration: row.decoration,
        code: row.code,
        code_lang: row.code_lang.clone(),
        directive: row.directive,
        directive_label: row.directive_label.clone(),
        media: row.media.clone(),
        // A tick, not an offset — reuse carries it as-is, like `code_lang`.
        task: row.task,
        leaf_directive: row.leaf_directive.clone(),
        heading: row.heading,
        // Structure, not offsets: a reused block's rows divide the same blocks
        // wherever the edit above moved them to.
        boundary: row.boundary,
    }
}

/// Advance a row's source offsets by `delta` in place — the suffix half of
/// [`build_spliced`], where the rows are already owned and only need shifting,
/// not copying.
fn shift_row_in_place(row: &mut VRow, delta: isize) {
    for g in &mut row.glyphs {
        g.src = (g.src as isize + delta) as usize;
    }
    row.end_src = (row.end_src as isize + delta) as usize;
}

/// Whether every source offset a block's rows carry falls inside the block's own
/// span — the precondition for reusing the block by a uniform offset shift. It
/// holds for well-formed blocks (their glyphs and row ends address bytes within
/// the block, synthetic glyphs point at the block start). It fails when a node
/// renders *outside* its block, which today means a malformed Markdown inline
/// node twig leaves with a degenerate `0..0` span: that content lands at a fixed
/// offset that doesn't move with the block. Such a block is re-rendered every
/// build instead of shifted, so the incremental map still matches a fresh one —
/// see [`build_cached`] and [`build_spliced`].
fn rows_within(rows: &[VRow], span: &Range<usize>) -> bool {
    rows.iter().all(|r| {
        r.end_src >= span.start
            && r.end_src <= span.end
            && r.glyphs.iter().all(|g| g.src >= span.start && g.src <= span.end)
    })
}

/// The document's rendered top-level blocks, as node indices in source order.
///
/// Not simply `doc`'s children, for two reasons. Frontmatter (a leading
/// `metadata` block) is document metadata rather than prose and is dropped, the
/// way [`Builder::blocks`] drops it. And a **footnote definition** (`[^1]: …`)
/// is not a child of `doc` at all: twig parses it as a root of its own, a
/// *sibling* of the document node with `parent == None`. A walk that starts at
/// `doc` therefore never reaches one, which is why a definition — and every
/// byte of its body — used to render as nothing at all. Merging the roots back
/// in by `span.start` puts each definition on screen exactly where it was
/// written, which is what keeps rows, stops, and offsets monotonic.
///
/// Only `footnote` roots are merged. twig also leaves stray orphan `str` nodes
/// parented to nothing (the `*` of an emphasis run, for one); those are already
/// rendered as part of the subtree that owns their bytes, and re-emitting them
/// here would double them.
fn top_level(nodes: &[FlatNode], doc: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut child = nodes[doc].first_child;
    while let Some(cid) = child {
        let n = &nodes[cid.0 as usize];
        if n.kind != Kind::Metadata {
            out.push(cid.0 as usize);
        }
        child = n.next_sibling;
    }
    out.extend(
        nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == Kind::Footnote && n.parent.is_none())
            .map(|(i, _)| i),
    );
    out.sort_by_key(|&i| nodes[i].span.start);
    out
}

/// The top-level blocks to hand [`build_cached`] / [`build_spliced`] — the
/// incremental path's twin of [`top_level`], which the two must agree with block
/// for block or the render paths diverge.
///
/// `child_spans(None)` gives `doc`'s children, which is all of them for an
/// ordinary document. A **footnote definition** is not one: twig parses `[^1]: …`
/// as a root beside `doc` with no parent, and indexes it at no offset either —
/// `node_at` inside its bytes answers `doc`, and a `query("footnote")` selector
/// finds nothing. Leaf used to discover them by marshalling the whole arena with
/// `nodes()` — the very cost the incremental path exists to avoid — behind a
/// byte-scan gate that gave documents with no `[^…]:` line a substring search
/// instead. twig 3.0's `definitions()` asks the library the question directly,
/// so both the marshal and the gate are gone.
///
/// Filtered to [`Kind::Footnote`]: `definitions()` also reports the *link*
/// reference definitions (`[foo]: /url`), which leaf has never rendered as
/// blocks and which are not this change's business to start rendering.
///
/// This is the one part of the render that needs an [`Editor`] rather than a
/// marshalled node array. The builders themselves stay editor-free; this only
/// prepares their input.
pub(crate) fn top_blocks(editor: &mut Editor) -> Vec<QueryMatch> {
    let mut top = editor.child_spans(None).unwrap_or_default();
    let notes = footnote_definitions(editor);
    if notes.is_empty() {
        return top;
    }
    top.extend(notes);
    // Source order — what every offset-keyed thing downstream (rows, stops, the
    // splice path's block-for-block match) is built to assume.
    top.sort_by_key(|m| m.span.start);
    top
}

/// Every `[^label]: …` definition in the document, in whatever order twig
/// reports them.
///
/// Filtered to [`Kind::Footnote`]: `definitions()` also reports the *link*
/// reference definitions (`[foo]: /url`), which leaf has never rendered as
/// blocks and which are not this function's business.
///
/// Empty when the document can't be walked, which leaves [`top_blocks`] with
/// the ordinary top-level children and [`crate::Doc::footnote_at_caret`] with an
/// undefined reference — in both cases the same answer as a document that has
/// no definitions, which is the right way to degrade.
pub(crate) fn footnote_definitions(editor: &mut Editor) -> Vec<QueryMatch> {
    let Ok(mut doc) = editor.document() else {
        return Vec::new();
    };
    doc.definitions()
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.kind == Kind::Footnote)
        .collect()
}

/// The label of the footnote definition starting at `start` — the `1` in
/// `[^1]: …`. twig gives the `footnote` node no label of its own (no `text`, no
/// `name`), and the bytes that spell it belong to no child node either — the
/// body `para` starts its *content* past them — so the source is the only place
/// to read it from. `None` when what's there isn't a definition after all.
pub(crate) fn footnote_label(source: &str, start: usize) -> Option<&str> {
    let rest = source.get(start..)?.strip_prefix("[^")?;
    let end = rest.find("]:")?;
    Some(&rest[..end])
}

/// Where the body of the footnote definition spanning `span` sits in `source` —
/// everything past the `[^1]:` marker, which is the part a reader actually wants
/// when they follow a reference.
///
/// Source bytes, verbatim but for the whitespace trimmed off each end: a note
/// that says `see *later*` answers with the asterisks in. Rendering that body is
/// a frontend's business the same way painting a [`Role`] is, and a caller that
/// wants it laid out already has the definition on screen where it was written.
///
/// The trim is what makes the common case read right — `[^1]: text` has a space
/// after the colon that belongs to the marker, not the note, and a definition's
/// span runs to the newline ending it.
///
/// A range rather than a slice because "go to note" needs the *position* as much
/// as the text, and it needs the position of the body specifically: a
/// definition's `[^1]:` marker is decoration the caret can't occupy (the rich
/// view draws it as `[1] ` and gives it no stop), so aiming a caret at the
/// definition's first byte lands it on the nearest real stop instead — which is
/// up in the paragraph *above* the note. The body's first byte is a stop, and is
/// where a reader following a reference wants to arrive anyway.
pub(crate) fn footnote_body_span(source: &str, span: Range<usize>) -> Option<Range<usize>> {
    let rest = source.get(span.clone())?.strip_prefix("[^")?;
    let marker = rest.find("]:")?;
    // `span.start` + `[^` + the label + `]:`.
    let after_marker = span.start + 2 + marker + 2;
    let raw = source.get(after_marker..span.end)?;
    let raw = &raw[..body_len(raw)];
    // Written as a start plus a length so an all-whitespace body lands on an
    // empty range at the end rather than an inverted one.
    let start = after_marker + (raw.len() - raw.trim_start().len());
    Some(start..start + raw.trim().len())
}

/// How much of a definition's span is actually the note, in bytes.
///
/// A definition's body is one paragraph plus its continuation lines: a
/// blank-line-separated block after it is a block of its own (an indented one
/// parses as a *code block*, not as more note), so the note ends at the first
/// line that isn't indented under it.
///
/// This has to be measured rather than taken from the span because twig's span
/// for a definition over-runs in djot — it reaches past the blank line into the
/// first byte of whatever follows, so `[^2a]: a note.` came back as
/// `"a note.\n\n["` and, worse, the *rows* the offsets named were the next
/// note's as well as this one's. A frontend showing one footnote would show two.
fn body_len(raw: &str) -> usize {
    let mut cut = raw.len();
    for (i, ch) in raw.char_indices() {
        if ch != '\n' {
            continue;
        }
        // Indented → the note continues onto this line. Anything else — another
        // definition, a paragraph, a blank line — is where it stops.
        let next = &raw[i + 1..];
        if !next.starts_with([' ', '\t']) {
            cut = i;
            break;
        }
    }
    cut
}

/// The label of the footnote *reference* spanning `span` — the `1` in `[^1]`.
///
/// The peer of [`footnote_label`] for the other half of the pair, and needed for
/// the same reason: a reference whose node carries neither a `content_span` nor
/// a `text` still spells its label plainly in the source. `None` when the bytes
/// aren't a reference after all.
pub(crate) fn footnote_reference_label(source: &str, span: Range<usize>) -> Option<&str> {
    let rest = source.get(span)?.strip_prefix("[^")?;
    let end = rest.find(']')?;
    Some(&rest[..end])
}

/// Where a heading's *content* starts — past the `#`s and the space the rich
/// view hides, for an ATX heading; the block's own start for a setext one (which
/// has no leading marker) and for a format that spells headings some other way.
///
/// Only an empty heading needs asking: with any content at all, the row ends on
/// its last glyph. Bounded to the heading's own first line so a marker-less
/// heading can't scan into the text under it.
fn heading_content_start(source: &str, span: &Range<usize>) -> usize {
    let end = span.end.min(source.len());
    let Some(line) = source.get(span.start..end) else { return span.start };
    let line = line.split('\n').next().unwrap_or("");
    let hashes = line.len() - line.trim_start_matches('#').len();
    if hashes == 0 {
        return span.start;
    }
    let after = &line[hashes..];
    span.start + hashes + (after.len() - after.trim_start_matches([' ', '\t']).len())
}

struct Builder<'a> {
    nodes: &'a [FlatNode],
    /// The document source, consulted to place blank-line rows at the source
    /// offsets the caret should occupy on them (the AST drops blank lines).
    source: &'a str,
    /// The word-wrap column budget, or `None` to emit each block as a single
    /// unwrapped row (the frontend wraps).
    wrap: Option<usize>,
    rows: Vec<VRow>,
    /// Built alongside `rows`, never instead of them — see [`TableInfo`].
    tables: Vec<TableInfo>,
    /// The end offset of the last content emitted — the anchor for blank
    /// separator rows so the caret never snaps onto one.
    last_off: usize,
    /// How many rows each block image reserves, keyed by its destination — the
    /// frontend's per-image height, threaded in from [`crate::Doc::set_media_rows`]
    /// so [`Builder::block_media`] can size the placeholder without core doing any
    /// I/O. A destination absent from the map (or a `0`/`1` entry) reserves the
    /// bare one-row placeholder, which is the whole-document default and what
    /// every existing test — passing an empty map — still gets.
    media_rows: &'a HashMap<String, usize>,
    /// The glyph a hard break renders as while the current inline run is built:
    /// a space in prose (a break folds into the flow the frontend wraps), but a
    /// newline (`\n`) inside a table cell, where a row is one source line and the
    /// only break it can carry is an explicit one that must show as a line of its
    /// own. Set around [`Builder::row_cells`] and otherwise left at `' '`.
    break_glyph: Cell<char>,
    /// Render a soft break (a bare newline inside a paragraph) as a line break
    /// where it was written, rather than folding it into the reflowed paragraph
    /// — the `LineFlow::Preserve` behaviour. A soft break emits a `'\n'` glyph
    /// (like a hard break in a cell), which [`Builder::emit_wrapped`] turns into
    /// a fresh visual row. `false` is the flowing-prose default. Inside a table
    /// cell (where `break_glyph` is already `'\n'`) it has no effect: a cell is
    /// one line and folds its own soft breaks regardless.
    preserve_soft: bool,
    /// The source byte range of the one line that should render its markup
    /// *raw* — the caret's line under `MarkupMode::Full` (see
    /// [`crate::Doc::reveal_line`]). `None` in every other mode and view, which
    /// is the delimiters-always-hidden behaviour every build had before the
    /// preference existed.
    ///
    /// Read only by [`Builder::revealed`], which every delimiter-bearing arm of
    /// [`Builder::inline`] consults. A range rather than a bare caret offset
    /// because the decision is per-*node*, not per-caret: a node is revealed
    /// when its span meets this line, so `*em*` shows both its asterisks even
    /// with the caret at one end of it.
    reveal: Option<Range<usize>>,
}

impl Builder<'_> {
    /// Whether `span` belongs to the line that is showing its raw markup. True
    /// only when a reveal line is set (`MarkupMode::Full`) and the two ranges
    /// actually meet.
    ///
    /// Touching at an endpoint counts: an emphasis ending exactly where the line
    /// does is on that line, and a zero-length reveal range (the caret alone on
    /// a blank line) still meets a node that starts there. The test is
    /// deliberately generous — the failure it avoids is revealing one delimiter
    /// of a pair while hiding the other, which looks like corruption rather than
    /// like markup.
    fn revealed(&self, span: &Range<usize>) -> bool {
        self.reveal
            .as_ref()
            .is_some_and(|r| span.start <= r.end && r.start <= span.end)
    }

    /// The `(opening, closing)` source byte ranges of a node's delimiters — the
    /// bytes its `span` holds that its `content_span` doesn't.
    ///
    /// This is how *every* inline delimiter is recovered, rather than a table of
    /// spellings per kind: twig gives `*em*` a span of `13..17` and a content
    /// span of `14..16`, so the gaps at each end are the delimiters, whatever
    /// they happen to be. That matters because one kind has many spellings —
    /// `*em*` and `_em_` are both emphasis, `` `x` `` and ``` ``x`` ``` both
    /// verbatim — and re-deriving the text from the source is the only way to
    /// show back what the author actually typed. It also gets a link's
    /// asymmetric `[` / `](dest)` right for free.
    ///
    /// `None` when the node has no content span, or when content and span
    /// coincide (nothing was elided, so there is nothing to reveal).
    fn delims(&self, id: usize) -> Option<(Range<usize>, Range<usize>)> {
        let node = &self.nodes[id];
        let content = node.content_span.clone()?;
        let span = node.span.clone();
        // A content span that escapes its own node's span means the two are
        // describing different things; reveal nothing rather than slice wildly.
        if content.start < span.start || content.end > span.end {
            return None;
        }
        let (open, close) = (span.start..content.start, content.end..span.end);
        // A delimiter that spans a newline isn't this line's to reveal — a setext
        // heading's `\n=====` underline is the case that arises in practice. It
        // would also inject a `'\n'` glyph, which `emit_wrapped` reads as a hard
        // row break, so the row would split where the author wrote no break.
        let multiline = |r: &Range<usize>| self.source.get(r.clone()).is_some_and(|s| s.contains('\n'));
        if multiline(&open) || multiline(&close) {
            return None;
        }
        (!open.is_empty() || !close.is_empty()).then_some((open, close))
    }

    /// Emit the source bytes of `range` as revealed markup — real glyphs, each
    /// mapped to its own source byte and each a caret stop, so a delimiter shown
    /// is a delimiter that can be selected, edited and deleted like any other
    /// text. Styled [`Role::Delimiter`] on top of the run's own style, which is
    /// how a frontend tells scaffolding from prose and dims it.
    ///
    /// Deliberately *not* [`push_escaped_text`]: this is raw source, not parsed
    /// text, so there is no escape-driven drift between the two to correct.
    fn push_delim(&self, out: &mut Vec<Glyph>, range: &Range<usize>, base: Style) {
        let Some(text) = self.source.get(range.clone()) else {
            return;
        };
        push_text(out, text, range.start, base.role(Role::Delimiter));
    }

    /// Render an inline node's children wrapped in its raw delimiters when the
    /// node is on the revealed line, and bare (delimiters resolved away) when it
    /// isn't — the shared body of every delimiter-bearing arm of
    /// [`inline`](Self::inline).
    ///
    /// `style` is the resolved styling the content still gets in *both* modes:
    /// revealing `*em*` shows the asterisks *and* keeps the text italic, the
    /// live-preview behaviour. Showing the markup is not the same as turning the
    /// rendering off — that is what [`crate::View::Source`] is for.
    fn inline_delimited(&self, id: usize, style: Style, out: &mut Vec<Glyph>) {
        let show = self.revealed(&self.nodes[id].span).then(|| self.delims(id)).flatten();
        if let Some((open, _)) = &show {
            self.push_delim(out, open, style);
        }
        self.recurse(id, style, out);
        if let Some((_, close)) = &show {
            self.push_delim(out, close, style);
        }
    }

    fn children(&self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut c = self.nodes[id].first_child;
        while let Some(cid) = c {
            out.push(cid.0 as usize);
            c = self.nodes[cid.0 as usize].next_sibling;
        }
        out
    }

    /// Render a node's block children, a blank separator between each. `tight`
    /// suppresses the *fabricated* separator between adjacent children that share
    /// a source line boundary — a tight list item and the sub-list nested in it —
    /// while a real blank source line between them still opens a gap.
    fn blocks(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph], tight: bool) {
        // Frontmatter (a leading `metadata` block) is document metadata, not
        // prose: hide it entirely in the rich-text view. Skipping it here means
        // no phantom blank rows for its lines and no separator before the first
        // real block — the document opens straight into its content.
        let kids: Vec<usize> = self
            .children(id)
            .into_iter()
            .filter(|&c| self.nodes[c].kind != Kind::Metadata)
            .collect();
        let mut above: Option<BlockClass> = None;
        for (i, child) in kids.into_iter().enumerate() {
            let below = BlockClass::from_node_kind(&self.nodes[child].kind);
            if let Some(above) = above {
                self.emit_separators_before(
                    self.nodes[child].span.start,
                    pc,
                    !tight,
                    Boundary { above, below },
                );
            }
            let first = if i == 0 { pf } else { pc };
            self.block(child, first, pc);
            above = Some(below);
        }
    }

    /// Render an explicit, ordered list of top-level blocks — [`Builder::blocks`]
    /// for a walk that isn't "the children of one node". The document's top level
    /// no longer is: a footnote definition is a root beside `doc`, not under it,
    /// and [`top_level`] merges it into this list by source position.
    ///
    /// The separator between blocks is spelled by the same
    /// [`Builder::emit_separators_before`] the incremental top-level walk in
    /// [`build_cached`] uses, so the two paths can't drift on how a boundary
    /// looks.
    fn top_blocks(&mut self, ids: &[usize]) {
        for (i, &child) in ids.iter().enumerate() {
            let below = BlockClass::from_node_kind(&self.nodes[child].kind);
            if i > 0 {
                let above = BlockClass::from_node_kind(&self.nodes[ids[i - 1]].kind);
                self.emit_separators_before(
                    self.nodes[child].span.start,
                    &[],
                    true,
                    Boundary { above, below },
                );
            }
            self.block(child, &[], &[]);
        }
    }

    /// Emit the blank separator row(s) that sit between a block ending at the
    /// current `last_off` and the next block starting at `next_start`, wearing
    /// the continuation prefix `pc`. Shared by [`Builder::blocks`] and the
    /// incremental top-level walk so the two can't drift on how a boundary is
    /// spelled.
    ///
    /// The blank line(s) between two blocks are real caret stops, each needing
    /// its *own* source offset — one strictly past the previous block's content,
    /// else it collides with that block's last row and `pos_of_offset`
    /// (first-match-wins) would resolve the caret onto the wrong row, pinning
    /// downward motion there.
    ///
    /// One row *per* blank source line, not a single collapsed separator: an
    /// empty paragraph opened between two blocks (Enter in the gap,
    /// `…\n\n\n\n…`) must be a navigable empty row, not vanish — else the caret
    /// in it snaps onto the *next* block's start and Enter looks like it did
    /// nothing.
    fn emit_separators_before(
        &mut self,
        next_start: usize,
        pc: &[Glyph],
        synthetic: bool,
        boundary: Boundary,
    ) {
        let mut offs = self.blank_rows_between(self.last_off, next_start);
        if offs.is_empty() {
            if !synthetic {
                // A tight list item's own text sits directly above the sub-list
                // nested in it — no fabricated gap. The "breathe" row belongs
                // between free-standing blocks, not between an item and its
                // child list, which the source writes on the very next line. A
                // real blank source line (a loose list) still lands a gap below,
                // because `blank_rows_between` found it and we never reach here.
                return;
            }
            // A tight gap with no blank line (e.g. a heading directly above its
            // text): keep the one conventional separator row so blocks still
            // breathe, as they always have.
            offs.push(self.blank_line_offset(self.last_off, next_start));
        }
        let last = offs.len() - 1;
        for (k, end_src) in offs.into_iter().enumerate() {
            // Only the drawn-only rows carry the boundary: the navigable blank
            // lines between them (and every blank line under preserve-soft flow)
            // are somewhere text can go, not a gap between blocks, and a frontend
            // that shrank one would be shrinking a line the author is typing on.
            let drawn = !self.preserve_soft && (k == 0 || k == last);
            // The blank line a boundary is *drawn* with isn't a place text can
            // go. The first one closes the block above and the last one opens the
            // block below — with a single blank line, the usual case, doing both
            // at once. Typing on either just continues the paragraph it abuts,
            // since the blank line it would need to be a paragraph of its own is
            // the very line being typed on. So they're a gap, like a table's
            // border: drawn, clickable, never a caret's home.
            //
            // The lines *between* them are the real ones. That's what Enter
            // opens: it inserts a paragraph break (`\n\n`), which leaves a blank
            // line spare on each side and the caret on the navigable line
            // between them.
            //
            // Preserve flow is the exception: there a bare `\n` is a visible line
            // break the author edits directly, so a lone blank line *is* a caret
            // home — typing on it makes the soft break the mode exists to show,
            // and Enter at a line's end lands the caret on exactly this row. So no
            // separator is drawn-only; every blank line is navigable.
            self.rows.push(VRow {
                glyphs: pc.to_vec(),
                end_src,
                decoration: drawn,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                boundary: drawn.then_some(boundary),
            });
        }
    }

    fn block(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "doc" | "section" => self.blocks(id, pf, pc, false),
            "heading" => {
                // A heading whose only visible content is a single image — a
                // banner set in an `<h1>` (`<h1><picture><img></picture></h1>`),
                // or `# ![](banner.png)` — is a block picture, not text. Render
                // it as one; anything with real heading text falls through.
                if let Some((m, kind)) = self.media_only(id) {
                    self.block_media(m, kind, id, pf);
                    return;
                }
                let level = node.level.unwrap_or(1);
                let style = heading_style(level);
                let mut glyphs = Vec::new();
                // On the revealed line the `# ` comes back as real, editable
                // text in front of the heading. Only the opening marker: a
                // closing `#`-run (`## title ##`) is covered by the same
                // `delims` pair, and a setext underline is excluded there for
                // being on another line entirely.
                if let Some((open, close)) = self.revealed(&node.span).then(|| self.delims(id)).flatten() {
                    self.push_delim(&mut glyphs, &open, style);
                    glyphs.extend(self.inline_children_with_trailing(id, style));
                    self.push_delim(&mut glyphs, &close, style);
                } else {
                    glyphs = self.inline_children_with_trailing(id, style);
                }
                // An *empty* heading — `# ` with nothing typed after it, which is
                // what the toolbar's H1 leaves on a blank line — has no glyph for
                // its row to end on, so the fallback below is the row's whole
                // extent: its only caret stop, and the offset every row after it
                // is measured from. The block's start is the wrong answer for
                // both, because it sits *in front of* the `# ` the rich view
                // hides: the caret drew (and typed) before the hashes, and the
                // rows below inherited an offset short by the marker's length,
                // which put the caret on one of them the moment the heading grew
                // text. Its content's start is where the caret belongs.
                let home = heading_content_start(self.source, &node.span);
                let first = self.rows.len();
                self.emit_wrapped(glyphs, home, pf, pc);
                // Stamp the level on every row the heading just emitted — a
                // wrapped heading's continuation rows as much as its first, and
                // an empty one's single glyphless row, which is the whole point
                // (see [`VRow::heading`]).
                for row in &mut self.rows[first..] {
                    row.heading = Some(level.min(255) as u8);
                }
            }
            "block_quote" => {
                let gutter = synth("│ ", Role::QuoteGutter, node.span.start);
                let f = concat(pf, &gutter);
                let c = concat(pc, &gutter);
                self.blocks(id, &f, &c, false);
            }
            // A generic `:::name{.class}` fenced-div container (twig's
            // `directive`, container form). Core is agnostic of `name` — it's
            // the host app's vocabulary (diaryx's `vis` for audience
            // visibility, say) and isn't available here regardless: twig only
            // threads an `element`'s tag name through `FlatNode::name`, not a
            // directive's own identifier. Every row gets marked `directive` (a
            // frontend draws a tinted panel around each maximal run, the
            // `code`/`code_block` recipe) and the first row carries a label —
            // the way a code fence's language rides only its first row.
            //
            // The label reads BOTH attribute conventions diaryx content
            // actually uses: twig's own dot-prefixed classes (`{.public
            // .family}`, one combined `class` attr) and bare pandoc-style
            // words with no leading dot (`{public family}` — the syntax
            // `diaryx_core::visibility`'s hand-rolled publish-time filter and
            // apps/web's directive serializer both write; twig parses each
            // bare word as its own attribute with an empty value, per
            // `languages/markdown/attributes.zig`). Reading only `.class`
            // would leave every *existing* diaryx `:::vis{...}` block
            // unlabeled.
            // Only the *container* form is the panel below. A `text` directive
            // is inline and never reaches the block walker (see `is_inline`); a
            // `leaf` one is a standalone block with no body, drawn as a
            // placeholder the way an image is.
            "container"
                if container_is_directive(node)
                    && node.directive_form == Some(DirectiveForm::Leaf) =>
            {
                self.block_directive(id, pf);
            }
            "container" if container_is_directive(node) => {
                let label = directive_attr_label(&node.attrs);
                let start_row = self.rows.len();
                self.blocks(id, pf, pc, false);
                for (i, row) in self.rows[start_row..].iter_mut().enumerate() {
                    row.directive = true;
                    if i == 0 {
                        row.directive_label = label.clone();
                    }
                }
            }
            "bullet_list" | "ordered_list" | "task_list" => {
                let ordered = node.kind == Kind::OrderedList;
                let mut item_no = 0usize;
                let kids = self.children(id);
                for (i, child) in kids.iter().copied().enumerate() {
                    let kind = &self.nodes[child].kind;
                    if *kind == Kind::ListItem || *kind == Kind::TaskListItem {
                        let start = self.nodes[child].span.start;
                        item_no += 1;
                        // A task item's box replaces the bullet rather than
                        // joining it. The `[ ] ` that spells it is markup twig
                        // has already consumed — the item's paragraph *content*
                        // starts past it — so without a drawn box a task item
                        // was indistinguishable from a plain bullet, ticked or
                        // not. `☐`/`☑` is the marker for the same reason `•` is:
                        // it stands where the source's own marker stands. Which
                        // way it faces is `checked`, straight off the node.
                        let checked = self.nodes[child].checked;
                        let marker = match (checked, ordered) {
                            (Some(true), _) => "☑ ".to_string(),
                            (Some(false), _) => "☐ ".to_string(),
                            (None, true) => format!("{item_no}. "),
                            (None, false) => "• ".to_string(),
                        };
                        let bullet = synth(&marker, Role::ListMarker, start);
                        let indent = synth(&" ".repeat(text_width(&marker)), Role::Body, start);
                        let first_row = self.rows.len();
                        self.block(child, &concat(pc, &bullet), &concat(pc, &indent));
                        // On the item's first row, the way `code_lang` rides the
                        // first row of its block.
                        if let (Some(c), Some(row)) = (checked, self.rows.get_mut(first_row)) {
                            row.task = Some(c);
                        }
                    } else {
                        // twig can nest a *following* top-level block as a direct
                        // child of the list rather than a sibling of it — e.g.
                        // `- item\n\n> quote` parses the block quote under the
                        // `bullet_list`. It isn't a list item, so render it de-nested:
                        // no bullet, at the list's own prefix, with the usual block
                        // separator — never `• │ quote`.
                        if i > 0 {
                            self.emit_separators_before(
                                self.nodes[child].span.start,
                                pc,
                                true,
                                Boundary {
                                    above: BlockClass::from_node_kind(&self.nodes[kids[i - 1]].kind),
                                    below: BlockClass::from_node_kind(&self.nodes[child].kind),
                                },
                            );
                        }
                        self.block(child, pc, pc);
                    }
                }
            }
            "list_item" | "task_list_item" => {
                // A childless item — the empty bullet you get the instant you
                // press Enter to open a new one — has no inner block to carry the
                // marker prefix or a caret home, so `blocks` would emit nothing
                // and the new bullet simply wouldn't appear until something was
                // typed into it. Emit the prefixed row itself, ending at a caret
                // stop just past the marker (the item's `span.end`), the way an
                // empty paragraph emits its one prefixed row via `emit_wrapped`.
                if self.children(id).is_empty() {
                    let home = self.nodes[id].span.end.min(self.source.len());
                    self.push_row_at(pf.to_vec(), home);
                } else {
                    // Tight: an item's text and the list nested under it butt
                    // together (`• a` / `  • b`), no fabricated blank row between —
                    // a loose item's real blank line still parts them.
                    self.blocks(id, pf, pc, true);
                }
            }
            // A footnote *definition* (`[^1]: the note`). It reaches this walker
            // only because [`top_level`] merges it back in — twig hangs it off no
            // parent at all, so a walk from `doc` never sees one and every byte
            // of its body used to render as nothing.
            //
            // Drawn as a hanging-indent item, the way a list item is: the marker
            // reads `[1] `, matching the `[1]` its references render as, so the
            // two can be paired by eye, and the body wraps under it. The marker
            // is synthetic decoration (one shared offset, never a caret stop) —
            // the `[^1]: ` that spells it in the source is markup, hidden like a
            // heading's `# `.
            "footnote" => {
                let (start, end) = (node.span.start, node.span.end);
                let source = self.source;
                let marker = format!("[{}] ", footnote_label(source, start).unwrap_or(""));
                let indent = " ".repeat(text_width(&marker));
                let f = concat(pf, &synth(&marker, Role::ListMarker, start));
                let c = concat(pc, &synth(&indent, Role::Body, start));
                if self.children(id).is_empty() {
                    // A definition with no body yet — the instant `[^1]: ` has
                    // been typed and nothing after it. `blocks` would emit
                    // nothing and the definition simply wouldn't appear, so emit
                    // the marker row itself with a caret home just past it,
                    // exactly as an empty list item does.
                    self.push_row_at(f, end.min(source.len()));
                } else {
                    self.blocks(id, &f, &c, false);
                }
            }
            "table" => self.table(id, pf, pc),
            "code_block" => {
                let style = Style::default().role(Role::Code);
                let text = node.text.clone().unwrap_or_default();
                let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
                // Each line at its own source offset, so the caret can walk the
                // code a character at a time like any other text. Where the
                // lines can't be lined up with the source there's no honest
                // offset to give, so the block maps coarsely to its start (and
                // stays a source-view job, as all of it once was).
                let offs = node
                    .content_span
                    .as_ref()
                    .and_then(|c| self.code_line_offsets(c, &lines));
                // The fence's info string, carried on the block's first row as
                // its language label (`None` for an indented block or a bare
                // fence). Kept on the row so it rides the block cache.
                let lang = code_language(self.source, node.span.start);
                for (i, raw) in lines.iter().enumerate() {
                    let at = offs.as_ref().map_or(node.span.start, |o| o[i]);
                    // No gutter glyph: the block is set apart by the border and
                    // tint a frontend draws around the whole run of `code` rows,
                    // not by a per-line mark. Just the block prefix (a list
                    // indent, a quote gutter) and the code text.
                    let mut glyphs: Vec<Glyph> = pf.to_vec();
                    push_text(&mut glyphs, raw, at, style);
                    // Explicitly past the line's *text*: a blank code line has no
                    // glyph, and any prefix's offset would put the row's end
                    // inside the next line.
                    self.push_row_at(glyphs, at + raw.len());
                    if let Some(row) = self.rows.last_mut() {
                        row.code = true;
                        if i == 0 {
                            row.code_lang = lang.clone();
                        }
                    }
                }
                // Anchor the block's end past its closing fence. Its last content
                // row ends at the last code line, before the ``` and the blank
                // line under it; without this the separator logic would count the
                // closing-fence line as its own blank row and open a phantom
                // second gap below the block.
                self.last_off = node.span.end;
            }
            "thematic_break" => {
                let full = self.wrap.unwrap_or(UNWRAPPED_RULE_WIDTH);
                let w = full.saturating_sub(prefix_width(pf)).max(4);
                let mut glyphs = pf.to_vec();
                for _ in 0..w {
                    glyphs.push(Glyph {
                        ch: '─',
                        style: Style::default().role(Role::Rule),
                        src: node.span.start,
                        // A rule is a block the caret can sit on, as it always
                        // has; it maps coarsely to the block's start.
                        stop: true,
                    });
                }
                self.push_row(glyphs, node.span.start);
            }
            // A block-level image node with no wrapping paragraph — a promoted
            // top-level HTML `<img>` lands as a direct `doc` child like this
            // (a Markdown `![](…)` comes wrapped in a `para`, handled below).
            "image" => self.block_media(id, MediaKind::Image, id, pf),
            // The same case for a promoted top-level `<video>`/`<audio>`, which
            // arrives as a generic `container` rather than a node kind of its
            // own. It can't be found by the `media_only` scan below the way a
            // wrapped one is: that scan looks at a wrapper's *children*, and here
            // the media element is itself the block.
            "container"
                if matches!(element_tag(node), Some("video") | Some("audio")) =>
            {
                let kind = match element_tag(node) {
                    Some("audio") => MediaKind::Audio,
                    _ => MediaKind::Video,
                };
                self.block_media(id, kind, id, pf);
            }
            _ => {
                // A container of blocks, or an inline-bearing paragraph.
                let kids = self.children(id);
                // A block-level image: a paragraph (or other wrapper — a
                // `<picture>`, an `<h1>` banner) whose only visible content is a
                // single `image` node. Render it as a placeholder row + record an
                // [`MediaInfo`] a capable frontend replaces. An image mixed with
                // real text or other images on the line isn't block-level and
                // falls through to the inline path below, still as its alt text.
                if let Some((m, kind)) = self.media_only(id) {
                    self.block_media(m, kind, id, pf);
                    return;
                }
                let inline =
                    !kids.is_empty() && kids.iter().all(|&c| is_inline(&self.nodes[c]));
                if inline || kids.is_empty() {
                    let glyphs = self.inline_children_with_trailing(id, Style::default());
                    if !glyphs.is_empty() {
                        self.emit_wrapped(glyphs, node.span.start, pf, pc);
                    }
                } else {
                    self.blocks(id, pf, pc, false);
                }
            }
        }
    }

    /// Render a table as a box-drawn grid: every column as wide as its widest
    /// cell, the header bold and ruled off, each cell padded to its column's
    /// alignment. This is the *default* monospace rendering (see
    /// [`VisualMap::rows`]); the same cells are also published structurally as
    /// [`TableInfo`], so a frontend that lays the grid out in its own units draws
    /// from there and skips the picture built here.
    ///
    /// The alignment comes from twig's `cell.alignment` — the delimiter row
    /// (`|:--|--:|`) that spells it out is consumed by the parser and leaves no
    /// node, so the snapshot is the only source for it.
    ///
    /// Borders and padding are *decoration*: they carry the source offset of the
    /// text they surround, so a click lands in that cell, but they're never
    /// caret stops — the caret steps cell-to-cell instead of into the box art.
    fn table(&mut self, id: usize, pf: &[Glyph], pc: &[Glyph]) {
        let node_end = self.nodes[id].span.end;
        // twig's shape is `[caption, row, row, …]`: the caption is always
        // present (usually empty in Markdown) and is not part of the grid.
        let row_ids: Vec<usize> = self
            .children(id)
            .into_iter()
            .filter(|&c| self.nodes[c].kind == Kind::Row)
            .collect();
        if row_ids.is_empty() {
            return;
        }
        // Lay every cell out first — the column widths depend on all of them.
        let grid: Vec<Vec<TableCell>> = row_ids.iter().map(|&r| self.row_cells(r)).collect();
        let heads: Vec<bool> = row_ids
            .iter()
            .map(|&r| self.nodes[r].head.unwrap_or(false))
            .collect();
        let cols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![0usize; cols];
        for row in &grid {
            for (c, cell) in row.iter().enumerate() {
                widths[c] = widths[c].max(cell_width(&cell.glyphs));
            }
        }
        // Every column at its widest cell is only the *wish*; a grid wider than
        // the surface has its far side hanging off the edge where no amount of
        // caret motion can reach it. Cut it down to what's actually there, and
        // let the cells wrap into the space they're given.
        if let Some(w) = self.wrap {
            fit_widths(&mut widths, w.saturating_sub(prefix_width(pc)));
        }

        // Where the picture starts, so a frontend drawing its own grid knows
        // which rows to skip. Recorded before the first border goes down.
        let rows_start = self.rows.len();

        let anchor = grid[0].first().map(|c| c.start).unwrap_or(node_end);
        self.push_rule(&rule_text(&widths, '┌', '┬', '┐'), anchor, pf);
        for (ri, row) in grid.iter().enumerate() {
            self.push_table_row(row, &widths, pc);
            // The rule under the header: only where the head actually ends.
            let ends_head = heads[ri] && heads.get(ri + 1) == Some(&false);
            if ends_head {
                let next = grid[ri + 1].first().map(|c| c.start).unwrap_or(node_end);
                self.push_rule(&rule_text(&widths, '├', '┼', '┤'), next, pc);
            }
        }
        self.push_rule(&rule_text(&widths, '└', '┴', '┘'), node_end, pc);

        // The same cells the picture above was drawn from, published unwrapped
        // and unpadded for a frontend that lays them out in pixels.
        self.tables.push(TableInfo {
            rows_span: rows_start..self.rows.len(),
            end_src: node_end,
            // The *continuation* prefix: `pf` opens the block and only its first
            // row wears it, but every row of a grid is a continuation of the
            // block the table sits in.
            prefix: pc.to_vec(),
            grid: grid
                .into_iter()
                .zip(heads)
                .map(|(cells, head)| TableRow { head, cells })
                .collect(),
        });
        // The table's own end anchors whatever separator follows it; the border
        // rows deliberately don't move `last_off` (they hold no content).
        self.last_off = node_end;
    }

    /// One row of laid-out cells, in column order.
    fn row_cells(&self, row: usize) -> Vec<TableCell> {
        // A cell is one source line, so a break within it is an explicit line
        // break (an inline `<br>`) that must render as a line of its own — not the
        // flow-folding space a break is in prose.
        self.break_glyph.set('\n');
        let cells = self
            .children(row)
            .into_iter()
            .filter(|&c| self.nodes[c].kind == Kind::Cell)
            .enumerate()
            .map(|(col, c)| {
                let n = &self.nodes[c];
                let style = if n.head.unwrap_or(false) {
                    Style::default().bold()
                } else {
                    Style::default()
                };
                // A cell's own `span` is the whole row; only `content_span` bounds
                // its text. An EMPTY cell has no `content_span` at all — twig
                // records no interior for it — so both offsets would fall back to
                // the row's start (before its first `│`), where every empty cell
                // in the row collapses onto the same spot and a click or caret
                // there types *before* the table. Derive the cell's own interior
                // from the row source and this cell's column instead, so each
                // empty cell has a distinct, editable caret home.
                let span = n.content_span.clone().unwrap_or_else(|| {
                    let off = empty_cell_offset(
                        &self.source[n.span.start.min(self.source.len())..n.span.end.min(self.source.len())],
                        n.span.start,
                        col,
                    );
                    off..off
                });
                TableCell {
                    glyphs: self.inline_children(c, style),
                    start: span.start,
                    end: span.end,
                    align: n.alignment.unwrap_or(Alignment::Default),
                }
            })
            .collect();
        self.break_glyph.set(' ');
        cells
    }

    /// A horizontal rule between/around rows — entirely decoration.
    fn push_rule(&mut self, text: &str, src: usize, prefix: &[Glyph]) {
        let glyphs = concat(prefix, &synth(text, Role::Rule, src));
        self.rows.push(VRow {
            glyphs,
            end_src: src,
            decoration: true,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            media: None,
                task: None,
            leaf_directive: None,
            heading: None,
            boundary: None,
        });
    }

    /// One `│ a │ b │` row of the grid: real cell text between decoration.
    ///
    /// A row of cells is not a row of the screen — a cell wrapped to its column
    /// spans several, each one `│`-divided across the full width so the grid
    /// stays square. Cells in the same row are laid out independently and run
    /// out at their own heights; a column that has run dry pads out as
    /// decoration while its neighbours keep going.
    fn push_table_row(&mut self, cells: &[TableCell], widths: &[usize], prefix: &[Glyph]) {
        let fallback = cells.last().map(|c| c.end).unwrap_or(0);
        let laid: Vec<Vec<Vec<Glyph>>> = cells
            .iter()
            .enumerate()
            .map(|(ci, c)| wrap_glyphs(&c.glyphs, widths.get(ci).copied().unwrap_or(0)))
            .collect();
        let height = laid.iter().map(|l| l.len()).max().unwrap_or(1).max(1);

        for j in 0..height {
            let mut glyphs = prefix.to_vec();
            for (ci, &w) in widths.iter().enumerate() {
                let cell = cells.get(ci);
                let line = laid.get(ci).and_then(|l| l.get(j));
                // The divider before this column belongs to the cell it
                // introduces, so clicking it lands in that cell — on this line
                // of it, which is what's next to the divider being clicked.
                let at = line
                    .and_then(|l| l.first().map(|g| g.src))
                    .or_else(|| cell.map(|c| c.start))
                    .unwrap_or(fallback);
                glyphs.extend(synth("│", Role::Rule, at));
                match (cell, line) {
                    (Some(cell), Some(line)) => {
                        let pad = w.saturating_sub(glyphs_width(line));
                        let (lead, trail) = match cell.align {
                            Alignment::Right => (pad, 0),
                            Alignment::Center => (pad / 2, pad - pad / 2),
                            Alignment::Left | Alignment::Default => (0, pad),
                        };
                        // Every line renders at least one space after its text
                        // (the gutter before `│`), so there is always somewhere
                        // to put the "after the last character" caret a line
                        // needs. It's the one padding glyph that is a stop: on
                        // the cell's last line that's the cell's end, and on any
                        // other it's the space the wrap consumed.
                        let last = laid[ci].len() == j + 1;
                        let end = match last {
                            true => cell.end,
                            false => line
                                .last()
                                .map(|g| g.src + g.ch.len_utf8())
                                .unwrap_or(cell.end),
                        };
                        glyphs.extend(synth(&" ".repeat(lead + 1), Role::Body, at));
                        glyphs.extend(line.iter().cloned());
                        glyphs.push(Glyph { ch: ' ', style: Style::default(), src: end, stop: true });
                        glyphs.extend(synth(&" ".repeat(trail), Role::Body, end));
                    }
                    // A ragged row, or a column whose cell ended higher up: pad
                    // it out so the grid stays square.
                    _ => {
                        let at = cell.map(|c| c.end).unwrap_or(fallback);
                        glyphs.extend(synth(&" ".repeat(w + 2), Role::Body, at));
                    }
                }
            }
            glyphs.extend(synth("│", Role::Rule, fallback));
            // The row ends where its last stop does. A table row has no gap
            // between its final cell and the border, so inventing an end past
            // that would be a stop with nothing under it.
            let end_src = glyphs
                .iter()
                .rev()
                .find(|g| g.stop)
                .map_or(fallback, |g| g.src);
            self.rows.push(VRow {
            glyphs,
            end_src,
            decoration: false,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            media: None,
                task: None,
            leaf_directive: None,
            heading: None,
            boundary: None,
        });
        }
    }

    /// Render a block-level image, video, or audio as one placeholder row: the
    /// `🖼 alt` / `🎬 alt` / `🔊 alt` label styled [`Role::Image`], every glyph
    /// mapped to the media's start offset and a caret stop there (they share the
    /// offset, so the stop table dedups them to a single home in front of it, as
    /// a rule's dashes do), and the row's end stop set past it so the caret can
    /// also rest after it. The row carries a [`MediaMark`] so [`media_spans`]
    /// publishes it as a [`MediaInfo`] a capable frontend replaces with the real
    /// picture or player; a plain surface paints the label as-is. `pf` is the
    /// block prefix (a list indent, a quote gutter) the row opens with, exactly
    /// as every other block honours it.
    fn block_media(&mut self, img: usize, kind: MediaKind, wrapper: usize, pf: &[Glyph]) {
        let node = &self.nodes[img];
        let start = node.span.start;
        let end = node.span.end;
        // An `image`'s URL is twig's `destination`; a `<video>`/`<audio>` is a
        // generic element, so its URL is the `src` attribute — and may be absent
        // entirely, the element naming its candidates in child `<source>`s.
        let destination = match kind {
            MediaKind::Image => node.destination.clone().unwrap_or_default(),
            MediaKind::Video | MediaKind::Audio => attr_of(node, "src").unwrap_or_default(),
        };
        let poster = match kind {
            MediaKind::Video => attr_of(node, "poster").unwrap_or_default(),
            MediaKind::Image | MediaKind::Audio => String::new(),
        };
        // The `<source>`s under the media element itself, not under `wrapper`: a
        // `<video>` is its own container, unlike an `<img>`, whose `<picture>`
        // alternatives are its *siblings* and so only reachable from the wrapper.
        let sources = match kind {
            MediaKind::Image => self.media_sources(wrapper),
            MediaKind::Video | MediaKind::Audio => self.media_sources(img),
        };
        let alt = self.image_alt(img);
        let sigil = kind.sigil();
        let label = if alt.is_empty() {
            // With no alt, name the file — but a `<video>` with neither `src` nor
            // alt has only its `<source>`s to be named by, so fall back to the
            // first candidate rather than labelling the row a bare sigil.
            let named = if destination.is_empty() {
                sources.first().map(|s| s.srcset.as_str()).unwrap_or_default()
            } else {
                &destination
            };
            format!("{sigil} {}", media_label(named))
        } else {
            format!("{sigil} {alt}")
        };
        let style = Style::default().role(Role::Image);
        let mut glyphs = pf.to_vec();
        for ch in label.chars() {
            glyphs.push(Glyph { ch, style, src: start, stop: true });
        }
        // How many rows the frontend wants for this picture: the label row plus
        // the blank fillers below it. Absent (a GUI that lays images out in
        // pixels, an image that didn't resolve, or a plain surface) means the
        // bare one-row placeholder.
        let rows = self.media_rows.get(&destination).copied().unwrap_or(1).max(1);
        // End past the image so the caret has a stop after it: the last glyph's
        // offset is the image *start*, not its extent, so `push_row`'s
        // last-glyph rule would strand the end stop inside the markup.
        self.push_row_at(glyphs, end);
        if let Some(row) = self.rows.last_mut() {
            row.media = Some(MediaMark { kind, destination, sources, alt, poster, rows });
        }
        // Reserve the picture's remaining height as blank `decoration` rows: drawn
        // (so the frontend has the vertical room to paint the raster over them),
        // but holding no caret and contributing no stops — vertical motion steps
        // over them and the caret's only homes stay the stop in front of the image
        // and the one just past it, both on the label row above. They anchor at the
        // image's end offset so a click on the picture's lower half lands after it,
        // the nearest caret home. Mirrors how a table's box-rule rows reserve space
        // without ever holding the caret.
        for _ in 1..rows {
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src: end,
                decoration: true,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                boundary: None,
            });
        }
        self.last_off = end;
    }

    /// The `<picture>` alternatives inside block-image `wrapper`, in document
    /// order — every `<source>` element in its subtree. Empty when there's no
    /// `<picture>`. Each is a `<source>`'s `media` + `srcset`; core keeps them
    /// verbatim and picks none (see [`MediaSource`]). A `<source>` with no
    /// `srcset` is dropped (nothing to load); its `media` may be empty (an
    /// unconditional override), which a frontend treats as always-matching.
    ///
    /// It scans the wrapper's whole subtree (via the forward `first_child` /
    /// `next_sibling` links, the reliable ones) rather than the `<img>`'s parent,
    /// for two reasons. A `<picture>` reaches core in two shapes: twig promotes a
    /// block `<picture>` to an `element(picture)` wrapping `[source, img]`, but
    /// leaves an inline one's tags as raw siblings — `[raw "<picture>", source,
    /// img, raw "</picture>"]` — so the `<source>`s sit at different depths in
    /// the two. And the editor's flat arena leaves a promoted inline node's
    /// `parent` back-pointer dangling on a phantom root, so only the wrapper
    /// (known at the call site) is a trustworthy anchor. A block image is the
    /// sole visible content of its wrapper, so every `<source>` under it is its
    /// picture's.
    fn media_sources(&self, wrapper: usize) -> Vec<MediaSource> {
        let mut out = Vec::new();
        self.collect_sources(wrapper, &mut out);
        out
    }

    fn collect_sources(&self, id: usize, out: &mut Vec<MediaSource>) {
        for c in self.children(id) {
            let node = &self.nodes[c];
            if node.name.as_deref() == Some("source") {
                // `<picture>` spells its candidate `srcset`, `<video>`/`<audio>`
                // spell it `src`. Both mean "the URL to load", so they normalise
                // onto one field; `srcset` wins where (illegally) both appear.
                let url = attr_of(node, "srcset").or_else(|| attr_of(node, "src"));
                if let Some(srcset) = url {
                    out.push(MediaSource {
                        media: attr_of(node, "media").unwrap_or_default(),
                        srcset,
                        mime: attr_of(node, "type").unwrap_or_default(),
                    });
                }
            }
            self.collect_sources(c, out);
        }
    }

    /// The single block-level media `id`'s subtree resolves to, or `None`.
    ///
    /// A wrapper is a block picture when the only *visible* thing under it is one
    /// image: whitespace-only text and structure-only elements (a `<picture>`'s
    /// `<source>`, which declares an alternate but paints nothing) don't count,
    /// and the search descends through wrapping elements (`<picture>`, a linking
    /// `<a>`). This is what makes `<p><img></p>`, a bare `<img>`, and
    /// `<h1><picture>…<img></picture></h1>` all render as one framed picture.
    /// Any real text, or a second image, means it isn't image-only — it falls
    /// back to inline rendering, where the image still shows as its alt text.
    ///
    /// [`FlatNode`]'s snapshot doesn't carry an element's tag name, so a
    /// `<source>` can't be skipped by name — but it needs no special case:
    /// contributing no image and no text, it's simply invisible to the scan.
    fn media_only(&self, id: usize) -> Option<(usize, MediaKind)> {
        let mut found = None;
        let mut count = 0usize;
        let mut has_text = false;
        self.scan_visual(id, &mut found, &mut count, &mut has_text);
        (count == 1 && !has_text).then(|| found.unwrap())
    }

    /// Walk `id`'s subtree tallying visible leaves for [`media_only`]: each
    /// image, `<video>`, or `<audio>` (remembering the last, counting the total)
    /// and whether any non-whitespace text appears. Media isn't descended into —
    /// an image's inline children are alt text, and a `<video>`'s are its
    /// no-support fallback and its `<source>` declarations, none of which is
    /// document content.
    ///
    /// [`media_only`]: Self::media_only
    fn scan_visual(
        &self,
        id: usize,
        found: &mut Option<(usize, MediaKind)>,
        count: &mut usize,
        has_text: &mut bool,
    ) {
        for c in self.children(id) {
            let node = &self.nodes[c];
            match node.kind.as_str() {
                "image" => {
                    *found = Some((c, MediaKind::Image));
                    *count += 1;
                }
                // A `<video>`/`<audio>` reaches core as a generic `container`
                // (twig gives neither a semantic node, so `html_elements`
                // promotion leaves the tag name on `name`). Counted as media and
                // *not* descended into, so its `<source>` children and its
                // "your browser does not support…" fallback text neither add a
                // second count nor make the block look like text.
                "container"
                    if matches!(
                        element_tag(node),
                        Some("video") | Some("audio")
                    ) =>
                {
                    let kind = match element_tag(node) {
                        Some("audio") => MediaKind::Audio,
                        _ => MediaKind::Video,
                    };
                    *found = Some((c, kind));
                    *count += 1;
                }
                // Text leaves: only non-whitespace counts as visible content.
                // (Twig keeps the whitespace `str`s between HTML tags — the
                // newlines and indentation inside a `<picture>` — as real nodes.)
                "str" | "smart_punctuation" | "verbatim" | "inline_math" => {
                    if node.text.as_deref().is_some_and(|t| !t.trim().is_empty()) {
                        *has_text = true;
                    }
                }
                // Structural breaks carry no visible glyph of their own.
                "soft_break" | "hard_break" | "non_breaking_space" => {}
                // Any other wrapper (emphasis, a link, a `<picture>`) is
                // transparent to the scan — descend into it.
                _ => self.scan_visual(c, found, count, has_text),
            }
        }
    }

    /// A leaf directive (`::name{…}`) as one placeholder row — the
    /// [`block_media`](Self::block_media) recipe, for the same reason: it is a
    /// block that renders as *a thing*, not as text, and the frontend paints
    /// whatever the host app's vocabulary makes of it.
    ///
    /// The row's glyphs are a `⧉ label` (or `⧉ name`) stand-in a plain surface
    /// paints as-is, every glyph anchored at the directive's start with a caret
    /// stop there, and the row ending past it so the caret can also rest after
    /// it. It carries a [`DirectiveMark`] for [`directive_spans`], and is marked
    /// [`directive`](VRow::directive) so a frontend already drawing the
    /// container form's panel frames this one identically for free.
    ///
    /// Before this, a leaf directive emitted no rows at all: it was invisible,
    /// held no caret, and vertical motion crossed a void where it stood.
    fn block_directive(&mut self, id: usize, pf: &[Glyph]) {
        let node = &self.nodes[id];
        let (start, end) = (node.span.start, node.span.end);
        let name = node.name.clone().unwrap_or_default();
        let attrs = node.attrs.clone();
        let label = self.image_alt(id); // its `[label]` children, flattened
        let shown = if label.is_empty() { &name } else { &label };
        let style = Style::default().role(Role::Image);
        let mut glyphs = pf.to_vec();
        for ch in format!("⧉ {shown}").chars() {
            glyphs.push(Glyph { ch, style, src: start, stop: true });
        }
        // End past the directive so the caret has a stop after it — the same
        // reason `block_media` anchors its row at the image's end.
        self.push_row_at(glyphs, end);
        if let Some(row) = self.rows.last_mut() {
            row.directive = true;
            row.leaf_directive = Some(DirectiveMark { name, attrs, label, rows: 1 });
        }
        self.last_off = end;
    }

    /// An image's alt text: the flattened text of its inline descendants (an
    /// image's children *are* its alt content), empty when it has none. Also a
    /// leaf directive's `[label]`, which is the same shape — inline children
    /// standing for the block.
    fn image_alt(&self, id: usize) -> String {
        let mut out = String::new();
        self.collect_text(id, &mut out);
        out
    }

    /// Append every descendant's `text` to `out`, in document order. Inline text
    /// (`str`) nodes are leaves, so a node never contributes both its own text and
    /// a child's — no double counting.
    fn collect_text(&self, id: usize, out: &mut String) {
        for c in self.children(id) {
            if let Some(t) = &self.nodes[c].text {
                out.push_str(t);
            }
            self.collect_text(c, out);
        }
    }

    fn inline_children(&self, id: usize, base: Style) -> Vec<Glyph> {
        let mut out = Vec::new();
        for c in self.children(id) {
            self.inline(c, base, &mut out);
        }
        out
    }

    /// [`inline_children`](Self::inline_children) plus any trailing whitespace the
    /// block carries past its inline content (see [`trailing_ws_glyphs`]). Used
    /// for the leaf inline blocks — paragraphs and headings — whose own `span`
    /// bounds exactly one line of text, so the trailing gap is theirs. *Not* for
    /// a table cell, whose `span` is the whole row and would swallow the
    /// delimiters and neighbours between it and the row's end.
    ///
    /// [`trailing_ws_glyphs`]: Self::trailing_ws_glyphs
    fn inline_children_with_trailing(&self, id: usize, base: Style) -> Vec<Glyph> {
        let mut out = self.inline_children(id, base);
        out.extend(self.trailing_ws_glyphs(id, base));
        out
    }

    /// Glyphs for whatever trailing whitespace a block's source carries past its
    /// last inline node — the space(s) at the end of `hello ` that Markdown and
    /// Djot drop from the `str` node as insignificant. twig still records them:
    /// a block's `content_span` ends at its last meaningful character while its
    /// `span` runs to the end of the line's text (before the terminating
    /// newline), so the gap between the two *is* that trailing whitespace.
    ///
    /// Emitting it as real caret-stop glyphs is what lets the caret be drawn
    /// past the last visible character. Without it, typing a space at the end of
    /// a paragraph moved the caret in the source but not on screen — the caret
    /// stuck on the last glyph until the next visible character reparsed the
    /// space into an interior `str` node that finally carried it.
    ///
    /// Restricted to spaces: only they are safe to synthesize one-cell-per-byte,
    /// and only they are what the parser silently strips. Anything else in the
    /// gap means the span accounting isn't what this assumes, so it's left alone.
    fn trailing_ws_glyphs(&self, id: usize, style: Style) -> Vec<Glyph> {
        let node = &self.nodes[id];
        let Some(content) = &node.content_span else {
            return Vec::new();
        };
        let (from, to) = (content.end, node.span.end);
        let Some(slice) = (from < to).then(|| self.source.get(from..to)).flatten() else {
            return Vec::new();
        };
        if slice.is_empty() || slice.bytes().any(|b| b != b' ') {
            return Vec::new();
        }
        slice
            .bytes()
            .enumerate()
            .map(|(i, _)| Glyph { ch: ' ', style, src: from + i, stop: true })
            .collect()
    }

    fn inline(&self, id: usize, base: Style, out: &mut Vec<Glyph>) {
        let node = &self.nodes[id];
        match node.kind.as_str() {
            "str" | "smart_punctuation" => push_escaped_text(
                out,
                node.text.as_deref().unwrap_or(""),
                node.span.clone(),
                &self.source,
                base,
            ),
            "soft_break" | "hard_break" | "non_breaking_space" => {
                // A break renders as a real, caret-navigable glyph — but twig
                // gives it no span of its own (`0..0`), so the offset comes from
                // the text in front of it: one *past* the last glyph, which is
                // the newline the break stands for. Past, not on: sharing the
                // previous glyph's offset would put two stops on one byte, and a
                // caret that can't change offset can't move.
                let src = if node.span.start != 0 {
                    node.span.start
                } else {
                    out.last().map(|g| g.src + g.ch.len_utf8()).unwrap_or(0)
                };
                // A *hard* break renders as this run's break glyph — a newline
                // inside a table cell (its own line), the same space in prose the
                // frontend re-wraps. A soft break normally folds into a space;
                // under `LineFlow::Preserve` it renders as a `'\n'` too, so the
                // author's line break shows where it was written. Never inside a
                // cell (`break_glyph` is `'\n'` there): a cell is one line and
                // folds its own soft breaks regardless.
                let ch = if node.kind == Kind::HardBreak {
                    self.break_glyph.get()
                } else if node.kind == Kind::SoftBreak && self.preserve_soft && self.break_glyph.get() == ' ' {
                    '\n'
                } else {
                    ' '
                };
                out.push(Glyph { ch, style: base, src, stop: true });
            }
            // A cell's only spelling for an in-line break is a raw `<br>`; read it
            // back as one (outside a cell it stays the literal text it falls to
            // below). The tag's bytes carry no stop of their own — the line it
            // ends stops just before it, the next just after.
            "raw_inline" if self.break_glyph.get() == '\n' && is_br(node.text.as_deref()) => {
                out.push(Glyph { ch: '\n', style: base, src: node.span.start, stop: true });
            }
            "emph" => self.inline_delimited(id, base.italic(), out),
            "strong" => self.inline_delimited(id, base.bold(), out),
            "mark" => self.inline_delimited(id, base.role(Role::Mark), out),
            "insert" => self.inline_delimited(id, base.underline(), out),
            "delete" => self.inline_delimited(id, base.strikethrough(), out),
            // The one pair whose whole meaning is *where the glyphs sit*. Drawn
            // in the surrounding style otherwise, so `^**2**^` stays bold and a
            // superscript inside a heading keeps the heading's role — which is
            // exactly why this is a `Baseline` and not a `Role`.
            "superscript" => self.inline_delimited(id, base.baseline(Baseline::Super), out),
            "subscript" => self.inline_delimited(id, base.baseline(Baseline::Sub), out),
            "verbatim" | "inline_math" => {
                // The interior begins at `content_span.start` — past however many
                // backticks the fence used, which `span.start + 1` only guessed
                // right for a single one. Fall back to that guess if it's absent.
                let at = node.content_span.as_ref().map_or(node.span.start + 1, |c| c.start);
                let style = base.role(Role::Code);
                // Not `inline_delimited`: verbatim has no child nodes to recurse
                // into — its content is its own `text` — so the fences bracket a
                // `push_text` instead. The fences themselves keep `Role::Code`'s
                // sibling treatment via `push_delim`'s role override.
                let show = self.revealed(&node.span).then(|| self.delims(id)).flatten();
                if let Some((open, _)) = &show {
                    self.push_delim(out, open, style);
                }
                push_text(out, node.text.as_deref().unwrap_or(""), at, style);
                if let Some((_, close)) = &show {
                    self.push_delim(out, close, style);
                }
            }
            // A text directive (`:name[label]{…}`) — the inline form of a generic
            // directive. Its `[label]` children are the visible text; the name and
            // the `{…}` attributes are the host app's vocabulary (diaryx's
            // `:vis[…]`) and stay hidden markup, exactly as a link's `](dest)` is.
            // Drawn in the surrounding style: a role of its own would need one
            // every frontend maps, and the bug this fixes is that the text was
            // invisible, not that it was unstyled.
            "container"
                if container_is_directive(node) && !self.children(id).is_empty() =>
            {
                self.recurse(id, base, out)
            }
            // No `[label]`, so there are no children to render and recursing
            // emitted *nothing*: the directive's bytes vanished from the document
            // and left no caret stop behind. What to draw instead turns on
            // whether the syntax looks deliberate.
            //
            // Bare `:word` almost never is. twig matches a colon followed by any
            // letter-led word (`scanTextDirective`, deliberately matching remark),
            // so ordinary prose is full of them — `:see below`, a `:smile:`
            // shortcode, a stray colon before a word. Those are prose, and prose
            // renders as itself: every byte visible, every byte a caret stop, so a
            // colon typed by accident can be seen and deleted. Hiding them behind
            // a placeholder would be the invisible-and-unreachable failure this
            // arm exists to fix, just wearing a nicer glyph.
            "container" if container_is_directive(node) && node.attrs.is_empty() => {
                let span = node.span.clone();
                push_text(out, self.source.get(span.clone()).unwrap_or(""), span.start, base);
            }
            // `{…}` attributes, though, are unmistakably deliberate — nobody
            // types `:vis{.family}` by accident, and diaryx writes exactly that
            // inline. So an attribute-bearing directive with no label draws as a
            // chip on `block_directive`'s recipe (`⧉ name attrs`, `Role::Image`),
            // the inline peer of the leaf form's placeholder row.
            //
            // Only the first glyph is a caret stop, and the whole chip shares the
            // directive's start offset: the caret treats it as one atomic thing
            // rather than walking hidden markup a byte at a time, and a paragraph
            // holding nothing but a chip still has a stop to be navigated to.
            "container" if container_is_directive(node) => {
                let start = node.span.start;
                let name = node.name.clone().unwrap_or_default();
                let shown = match directive_attr_label(&node.attrs) {
                    Some(attrs) if !name.is_empty() => format!("⧉ {name} {attrs}"),
                    Some(attrs) => format!("⧉ {attrs}"),
                    None => format!("⧉ {name}"),
                };
                let style = base.role(Role::Image);
                for (i, ch) in shown.chars().enumerate() {
                    out.push(Glyph { ch, style, src: start, stop: i == 0 });
                }
            }
            // A footnote reference (`[^1]`). The label bracketed is what a reader
            // needs — bare, `note1` reads as a typo rather than a reference — so
            // the `^` is hidden as the spelling artefact it is (a link's
            // `](dest)` goes the same way) and the brackets are kept as
            // decoration: one shared offset, never a caret stop, like a table's
            // borders, so the caret walks the label alone.
            //
            // Styled `Role::Link`: a reference *is* a link to its definition, and
            // every frontend already paints that role. A role of its own would
            // need one in each of them, and what a frontend needs to tell the two
            // apart is not a paint colour but an answer to "what does clicking
            // here do" — which is [`Doc::footnote_at_caret`]'s job, not a glyph's.
            //
            // Raised, though, because that a reference is *set* differently from
            // the prose it interrupts is exactly what makes it read as a
            // reference. `[1]` at body size reads as bracketed text.
            "footnote_reference" => {
                let style = base.role(Role::Link);
                // Revealed, the reference is just its source bytes: the `^` that
                // is normally elided comes back and every byte becomes a real
                // stop, so the brackets stop being decoration and start being
                // text. That's the whole point of the mode, and it replaces the
                // hand-built chip below rather than decorating it — including the
                // raised baseline, since what's on screen there is source, and
                // source is set as prose.
                if self.revealed(&node.span) {
                    self.push_delim(out, &node.span, style);
                    return;
                }
                let style = style.baseline(Baseline::Super);
                // The label's own span, so its glyphs map to their true bytes.
                // Absent one, it starts past the `[^` that opens the reference.
                let (label, at) = match &node.content_span {
                    Some(c) => (self.source.get(c.clone()).unwrap_or(""), c.start),
                    None => (node.text.as_deref().unwrap_or(""), node.span.start + 2),
                };
                out.push(Glyph { ch: '[', style, src: node.span.start, stop: false });
                push_text(out, label, at, style);
                out.push(Glyph {
                    ch: ']',
                    style,
                    src: node.span.end.saturating_sub(1),
                    stop: false,
                });
            }
            "link" | "url" | "email" => {
                let style = base.role(Role::Link);
                if self.children(id).is_empty() {
                    // A bare autolink (`<a@b.c>`, a naked URL): the destination
                    // *is* the visible text, so there is nothing elided to
                    // reveal and both modes draw the same thing.
                    push_text(out, node.destination.as_deref().or(node.text.as_deref()).unwrap_or("link"), node.span.start, style);
                } else {
                    // An inline link reveals asymmetrically — `[` before the
                    // label, `](dest)` after it — which the generic
                    // span-minus-content derivation already produces.
                    self.inline_delimited(id, style, out);
                }
            }
            _ => {
                if self.children(id).is_empty() {
                    if let Some(t) = &node.text {
                        push_text(out, t, node.span.start, base);
                    }
                } else {
                    self.recurse(id, base, out);
                }
            }
        }
    }

    fn recurse(&self, id: usize, style: Style, out: &mut Vec<Glyph>) {
        for c in self.children(id) {
            self.inline(c, style, out);
        }
    }

    /// Lay a block's inline `glyphs` into visual rows, prefixing the first with
    /// `pf` and the rest with `pc`. A preserved soft break arrives as a `'\n'`
    /// glyph (see the `soft_break` arm): a hard row boundary that splits the
    /// glyphs so each run lays out on its own and the author's line structure
    /// shows on screen. The `'\n'` is dropped from the row it closes and its
    /// source offset becomes that row's end stop — exactly how a table cell's
    /// in-line `<br>` is handled — so the caret can rest at the line's end
    /// without a zero-width control char leaking into what the frontends render.
    /// With no `'\n'` present (the folding default, and every build that isn't
    /// `LineFlow::Preserve`) there is one run and this is byte-identical to
    /// laying the glyphs out directly.
    fn emit_wrapped(&mut self, glyphs: Vec<Glyph>, block_start: usize, pf: &[Glyph], pc: &[Glyph]) {
        if !glyphs.iter().any(|g| g.ch == '\n') {
            self.emit_line(glyphs, block_start, pf, pc, None);
            return;
        }
        // Each run up to a '\n' is a line of its own: the first wears the block's
        // opening prefix, every later one the continuation prefix, and the break's
        // own offset ends the run's last row. The break glyph is dropped. A
        // trailing '\n' flushes its run and leaves nothing behind, so no spurious
        // blank row follows it.
        let mut run: Vec<Glyph> = Vec::new();
        let mut first = true;
        for g in glyphs {
            if g.ch == '\n' {
                let lead = if first { pf } else { pc };
                self.emit_line(std::mem::take(&mut run), block_start, lead, pc, Some(g.src));
                first = false;
            } else {
                run.push(g);
            }
        }
        if !run.is_empty() {
            let lead = if first { pf } else { pc };
            self.emit_line(run, block_start, lead, pc, None);
        }
    }

    /// Word-wrap a single line of `glyphs` (no interior line breaks) to the
    /// available width and push the visual rows, prefixing the first with `pf`
    /// and the rest with `pc`. `end`, when set, is the source offset that ends
    /// the line's final row — the offset of the break that terminated it, which
    /// the caller has already stripped from `glyphs`; when `None` the row ends
    /// just past its last glyph, as an unbroken block's does.
    fn emit_line(&mut self, glyphs: Vec<Glyph>, block_start: usize, pf: &[Glyph], pc: &[Glyph], end: Option<usize>) {
        // The line's final row ends at `end` when a break gave one, else just
        // past its last glyph (`push_row`'s default).
        let push_last = |b: &mut Self, row: Vec<Glyph>| match end {
            Some(e) => b.push_row_at(row, e),
            None => b.push_row(row, block_start),
        };

        // No column budget: emit the whole line as one row and let the frontend
        // wrap it at its own (pixel) width.
        let Some(width) = self.wrap else {
            let row = if glyphs.is_empty() { pf.to_vec() } else { concat(pf, &glyphs) };
            push_last(self, row);
            return;
        };

        // Split into words (maximal non-space runs), each carrying the space
        // glyph that followed it (so its source offset is preserved).
        let mut words: Vec<(Vec<Glyph>, Option<Glyph>)> = Vec::new();
        let mut word: Vec<Glyph> = Vec::new();
        for g in glyphs {
            if g.ch == ' ' {
                words.push((std::mem::take(&mut word), Some(g)));
            } else {
                word.push(g);
            }
        }
        if !word.is_empty() {
            words.push((word, None));
        }
        if words.is_empty() {
            // An empty block (or an empty preserved line) still occupies one
            // (prefixed) row.
            push_last(self, pf.to_vec());
            return;
        }

        let mut line: Vec<Glyph> = Vec::new();
        let mut used = 0usize;
        let mut first = true;
        for (w, space) in words {
            let avail = width
                .saturating_sub(prefix_width(if first { pf } else { pc }))
                .max(1);
            let cells = glyphs_width(&w);
            if used > 0 && used + cells > avail {
                let row = concat(if first { pf } else { pc }, &line);
                self.push_row(row, block_start);
                line = Vec::new();
                used = 0;
                first = false;
            }
            used += cells;
            line.extend(w);
            if let Some(sp) = space {
                used += 1;
                line.push(sp);
            }
        }
        let row = concat(if first { pf } else { pc }, &line);
        push_last(self, row);
    }

    /// The source offset of each line of a code block's `text`.
    ///
    /// `content` is the block's `content_span` — where twig says the body lives
    /// in the source, fences already excluded. Its lines run 1:1 with the
    /// rendered `text` lines, so no search is needed; each is anchored at the
    /// *end* of its source line, which places it past whatever indent `text` had
    /// stripped (a fenced block's fences, an indented one's leading spaces)
    /// without having to know how much there was.
    ///
    /// `None` when the body and the rendered lines don't line up — a coarse
    /// fallback the caller turns into the block's start offset.
    fn code_line_offsets(&self, content: &Range<usize>, lines: &[&str]) -> Option<Vec<usize>> {
        let mut src_lines: Vec<(usize, &str)> = Vec::new();
        let mut at = content.start;
        for l in self.source.get(content.start..content.end)?.split('\n') {
            src_lines.push((at, l));
            at += l.len() + 1;
        }
        if src_lines.len() != lines.len() {
            return None;
        }
        Some(
            lines
                .iter()
                .zip(&src_lines)
                .map(|(l, (start, sl))| start + sl.len().saturating_sub(l.len()))
                .collect(),
        )
    }

    fn push_row(&mut self, glyphs: Vec<Glyph>, fallback: usize) {
        // Step past the character the *source* holds at the last glyph's offset,
        // not past the glyph's own `ch`. The two agree for ordinary text, but a
        // glyph is not always the character it stands on: `synth` decoration and
        // a substituted run (an image's `⧉ label`) share one offset by design.
        // Trusting `ch` there yields an offset inside a multi-byte character,
        // which every later slice of `source` panics on.
        let end_src = glyphs
            .last()
            .map(|g| {
                let at = g.src.min(self.source.len());
                at + self.source[at..].chars().next().map_or(0, char::len_utf8)
            })
            .unwrap_or(fallback);
        self.push_row_at(glyphs, end_src);
    }

    /// Push a row with an explicit end stop, for content that knows its own
    /// extent better than its last glyph does.
    fn push_row_at(&mut self, glyphs: Vec<Glyph>, end_src: usize) {
        self.last_off = end_src;
        self.rows.push(VRow {
            glyphs,
            end_src,
            decoration: false,
            code: false,
            code_lang: None,
            directive: false,
            directive_label: None,
            media: None,
                task: None,
            leaf_directive: None,
            heading: None,
            boundary: None,
        });
    }

    /// The source offset the caret rests at on the blank line separating a block
    /// that ends at `prev_end` from the next block starting at `next_start`:
    /// just past the newline that terminates the previous block, but kept
    /// strictly before the next block so the offset is unique to this row.
    fn blank_line_offset(&self, prev_end: usize, next_start: usize) -> usize {
        let after_nl = self.source[prev_end..]
            .find('\n')
            .map_or(prev_end, |p| prev_end + p + 1);
        after_nl.min(next_start.saturating_sub(1)).max(prev_end)
    }

    /// The source offset of each blank row between a block ending at `prev_end`
    /// and content starting at `next_start` — one per blank source line. The
    /// first newline terminates the previous block's line; every line it opens up
    /// to (but not including) the line that holds `next_start` is a blank row the
    /// caret can occupy. Offsets are unique and ascending so `pos_of_offset`
    /// resolves each to its own row. Empty when the two blocks are tight (no
    /// blank line between them).
    fn blank_rows_between(&self, prev_end: usize, next_start: usize) -> Vec<usize> {
        // Spans aren't always in tidy source order (e.g. a block after
        // frontmatter can start *before* the previous block's rendered content
        // ends). There's no blank line to place then — fall back to the clamped
        // single separator (an empty return) rather than slicing an inverted
        // range.
        if next_start <= prev_end {
            return Vec::new();
        }
        let gap = &self.source[prev_end..next_start];
        let Some(nl) = gap.find('\n') else {
            return Vec::new();
        };
        // The line holding `next_start` belongs to the next block; blank rows
        // stop before it.
        let next_line_start = self.source[..next_start]
            .rfind('\n')
            .map_or(0, |p| p + 1);
        let mut offs = Vec::new();
        let mut start = prev_end + nl + 1;
        while start < next_line_start {
            offs.push(start);
            match self.source[start..next_start].find('\n') {
                Some(k) => start += k + 1,
                None => break,
            }
        }
        offs
    }

    /// Blank lines the user typed past the end of the last block (e.g. two
    /// `Enter`s to open a fresh paragraph) leave no AST node, so nothing renders
    /// and the caret appears stuck on the old line. Reconstruct one empty row
    /// per extra trailing newline from the source, each at its own offset, so
    /// the caret rides down onto the new line the moment it's created.
    ///
    /// `above` is the class of the last block in the document — the one this gap
    /// closes. A document with no blocks at all has nothing above these rows, and
    /// [`BlockClass::Paragraph`] is the honest answer there too: what they are is
    /// empty paragraphs, on both sides of the gap.
    fn emit_trailing_blank_lines(&mut self, above: BlockClass) {
        let last_end = self.rows.last().map_or(0, |r| r.end_src);
        if last_end >= self.source.len() {
            return;
        }
        // The first newline after the last content just terminates that line, so
        // a lone trailing `\n` (an ordinary file ending) opens no blank row. A
        // *second* newline opens an empty paragraph: render it the way a block
        // boundary is rendered — a blank spacer row, then the empty paragraph row
        // the caret rests on — so the just-pressed-Enter view already shows the
        // gap it will keep once text is typed, and typing doesn't shift the line
        // down. One row per trailing newline (each its own caret offset), the
        // last landing at the document end where the caret sits.
        let extra = self.source[last_end..].matches('\n').count();
        if extra < 2 {
            return;
        }
        for k in 1..=extra {
            self.rows.push(VRow {
                glyphs: Vec::new(),
                end_src: last_end + k,
                // As between two blocks: the first blank row is the gap that
                // closes the block above, not somewhere to type. Nothing follows
                // to need a gap of its own, though, so every row after it is a
                // real empty paragraph — the end of the document bounds the last
                // one the way a following block would. Preserve flow makes even
                // that first row navigable, as it does every blank line.
                decoration: !self.preserve_soft && k == 1,
                code: false,
                code_lang: None,
                directive: false,
                directive_label: None,
                media: None,
                task: None,
                leaf_directive: None,
                heading: None,
                // The one drawn row here is a block boundary like any other —
                // "rendered the way a block boundary is rendered" is the whole
                // point of it — so it says so, and a frontend spacing boundaries
                // spaces this one the same. The rows below it are navigable empty
                // paragraphs, not gaps.
                boundary: (!self.preserve_soft && k == 1).then_some(Boundary {
                    above,
                    below: BlockClass::Paragraph,
                }),
            });
        }
    }
}

// ── display width ────────────────────────────────────────────────────────────
//
// Two things a row can be counted in, and they are not the same number:
//
//   *glyphs*, one per codepoint — how the text is stored here, and what an
//   index into `VRow::glyphs` means; and
//   *columns*, one per terminal cell — where the text is drawn, and what every
//   `col` in this crate means.
//
// `你` is one glyph in two columns. Counting columns with `glyphs.len()` (or,
// in the source view, `chars().count()`) is the same number only for the ASCII
// that most fixtures are written in, and drifts one cell per wide character
// everywhere else — the caret drawn a column short of the text it types into.
// Everything below converts between the two; nothing else should have to.

/// The display width of `s` in terminal cells.
///
/// Measured per grapheme cluster, because that is the unit a surface advances
/// by: `👨‍👩‍👧` is five codepoints measuring 2 + 0 + 2 + 0 + 2 cells one at a
/// time, but the character they spell is drawn in 2. Both frontends already
/// measure it that way — ratatui asks `unicode-width` per cluster, and the GUI
/// asks its own text system — so the caret only lands where the text is if this
/// agrees with them.
pub fn text_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// One grapheme cluster of a laid-out row: the glyphs that spell it, and the
/// cells it is drawn in.
///
/// The cluster, not the glyph, is what has a width. A row's glyphs are one per
/// codepoint, so an accented letter or an emoji is several of them drawn in one
/// character's worth of cells — the glyph that opens the cluster claims those
/// cells, and the ones continuing it are drawn *inside* them rather than beside
/// them. It's the same cluster the stop table is built on: the opening glyph is
/// the one a caret can rest on, and so the only one whose column it can be
/// drawn at.
struct Cluster {
    /// Index of the glyph that opens it.
    glyph: usize,
    /// The display column it starts at.
    col: usize,
    /// How many cells it is drawn in. Zero for a cluster with no width of its
    /// own (a lone joiner), which therefore sits at no column at all.
    cells: usize,
}

/// Walk a row's glyphs as the clusters they spell, in column order.
fn clusters(glyphs: &[Glyph]) -> Vec<Cluster> {
    let text: String = glyphs.iter().map(|g| g.ch).collect();
    let mut out = Vec::new();
    let (mut glyph, mut col) = (0, 0);
    for cluster in text.graphemes(true) {
        let cells = text_width(cluster);
        out.push(Cluster { glyph, col, cells });
        // One glyph per codepoint, so a cluster spans exactly its own.
        glyph += cluster.chars().count();
        col += cells;
    }
    out
}

/// The display width of a run of glyphs.
fn glyphs_width(glyphs: &[Glyph]) -> usize {
    clusters(glyphs).last().map_or(0, |c| c.col + c.cells)
}

/// A cell's display width — the widest of its lines, since an in-cell `\n` break
/// splits it into several. Sizes the column that must hold every line.
fn cell_width(glyphs: &[Glyph]) -> usize {
    glyphs
        .split(|g| g.ch == '\n')
        .map(glyphs_width)
        .max()
        .unwrap_or(0)
}

/// Whether a raw inline HTML tag is a line break (`<br>`, `<br/>`, `<br />`,
/// case-insensitively) — the one tag a table cell reads as an in-cell break.
fn is_br(text: Option<&str>) -> bool {
    let Some(t) = text else { return false };
    matches!(
        t.trim().to_ascii_lowercase().replace(' ', "").as_str(),
        "<br>" | "<br/>"
    )
}

impl VRow {
    /// The row's width in display columns — and so the column of the caret
    /// placed past its last glyph, which is the rightmost column it can occupy.
    fn width(&self) -> usize {
        glyphs_width(&self.glyphs)
    }

    /// The display column glyph `i` is drawn at. Glyphs continuing a cluster
    /// report the column of the glyph that opened it, since that is where they
    /// are drawn; none of them is ever a stop, so no caret is placed by it.
    fn col_of_glyph(&self, i: usize) -> usize {
        clusters(&self.glyphs)
            .iter()
            .rev()
            .find(|c| c.glyph <= i)
            .map_or(0, |c| c.col)
    }

    /// The glyph drawn at display column `col`, or `None` past the row's last
    /// cell.
    ///
    /// A column landing on the *second* cell of a wide glyph resolves to that
    /// glyph: half a character is not a place to be, so clicking either cell of
    /// `你` means `你`, and the caret comes to rest at its start — the column it
    /// would be drawn at anyway. That rule is what makes the mapping invertible:
    /// every offset has one column, and every column has one offset.
    fn glyph_at_col(&self, col: usize) -> Option<usize> {
        clusters(&self.glyphs)
            .into_iter()
            .find(|c| col < c.col + c.cells)
            .map(|c| c.glyph)
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// The caret home inside an *empty* table cell (`col`, 0-based) of a row whose
/// source is `row_src` starting at byte `row_start`. twig gives an empty cell no
/// `content_span`, so its interior is read from the pipes: cell `col` lies
/// between the `col`-th and `col+1`-th unescaped `│`/`|`, and the home is one
/// space past the opening one — mimicking the `| ` padding a filled cell has,
/// and never at or past the closing pipe. So `|  |  |` gives the two cells
/// distinct, editable homes instead of both collapsing onto the row's start.
fn empty_cell_offset(row_src: &str, row_start: usize, col: usize) -> usize {
    let bytes = row_src.as_bytes();
    let mut pipes = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'|' && (i == 0 || bytes[i - 1] != b'\\') {
            pipes.push(i);
        }
    }
    match (pipes.get(col).copied(), pipes.get(col + 1).copied()) {
        (Some(open), Some(close)) => {
            let lo = open + 1; // just inside the opening pipe
            let hi = close.saturating_sub(1); // just inside the closing pipe
            let inside = if hi < lo { lo } else { (open + 2).clamp(lo, hi) };
            row_start + inside
        }
        (Some(open), None) => row_start + open + 1,
        _ => row_start,
    }
}

/// One laid-out table cell: its rendered text, the source range that text
/// occupies (`start`/`end` are the caret anchors decoration points at), and the
/// column alignment its padding honours.
///
/// `glyphs` is the cell's inline content *unwrapped* — the box-drawn rows wrap
/// it to a column width, but a frontend laying the grid out itself needs the
/// text before that decision was made.
#[derive(Clone)]
pub struct TableCell {
    pub glyphs: Vec<Glyph>,
    pub start: usize,
    pub end: usize,
    pub align: Alignment,
}

/// One row of a table's grid, as the document spells it — not as it's drawn.
#[derive(Clone)]
pub struct TableRow {
    /// A header row: drawn bold, and ruled off from the body below it.
    pub head: bool,
    pub cells: Vec<TableCell>,
}

/// A table's structure, published alongside the box-drawn rows that spell it.
///
/// The rows in [`VisualMap::rows`] are the *default monospace* picture of a
/// table: every border a `│`, every column a whole number of character cells.
/// That picture is exactly right on any monospace surface, and unfixable off one
/// — in a proportional font the `│`s of two rows land at different x and the grid
/// shears. So a frontend that draws its own geometry reads this instead: the
/// cells, their alignment, and which rows are the head, with no opinion about
/// how wide a column is or what a border looks like.
///
/// Both are always built. The TUI paints `rows` and ignores this; the GUI skips
/// `rows` for the span in `rows_span` and draws from here. They describe the
/// same cells, so the caret lands on the same offsets either way.
#[derive(Clone)]
pub struct TableInfo {
    /// The `VisualMap::rows` this table's picture occupies, borders included —
    /// what a frontend drawing its own table skips over.
    pub rows_span: Range<usize>,
    /// The source span of the table node, and the offset its trailing caret
    /// stop sits at.
    pub end_src: usize,
    /// The block prefix every row of this table carries — a blockquote's `│ `
    /// gutter, a list item's indent. Empty for a table at the top level.
    ///
    /// A frontend drawing its own grid has to render this and start the table
    /// past it, exactly as the picture does; a table nested in a quote that
    /// draws flush at the left margin has left the quote.
    pub prefix: Vec<Glyph>,
    pub grid: Vec<TableRow>,
}

/// A fenced or indented code block, named by the [`VisualMap::rows`] it occupies.
///
/// Unlike a table, the rows *are* the block's content — a frontend still paints
/// them, it just draws a border and a tinted background around the whole span
/// and lets the code inside scroll horizontally instead of wrapping. So this
/// carries only the row range; there's no structural alternative to the picture
/// the way [`TableInfo`] is one. Derived from [`VRow::code`] — see
/// [`code_block_spans`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBlockInfo {
    /// The contiguous run of [`VisualMap::rows`] this code block spans, blank
    /// code lines included.
    pub rows_span: Range<usize>,
    /// The block's language, from a fenced block's info string — what a frontend
    /// paints as a small label on the box (`` ```rust `` → `Some("rust")`).
    /// `None` for a fence written without one, or an indented block. Editing it
    /// goes through [`crate::Doc::set_code_language`], which re-finds the fence
    /// in the AST, so this stays a display string.
    pub lang: Option<String>,
}

/// A block-level image (`![alt](url)` on its own line), named by the single
/// [`VisualMap::rows`] row it occupies.
///
/// Like [`CodeBlockInfo`], the row *is* the block's default rendering — a plain
/// surface paints the `🖼 alt` placeholder glyphs as-is. An image-capable
/// frontend instead **skips the row in `rows_span`** and paints the resolved
/// picture there, exactly as it skips a [`TableInfo`]'s box-drawn rows. Derived
/// from [`VRow::image`] by [`media_spans`], so it survives the row reuse of
/// [`BlockCache`] and [`build_spliced`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaInfo {
    /// The [`VisualMap::rows`] rows this media's placeholder occupies — what a
    /// capable frontend replaces with the picture or player.
    pub rows_span: Range<usize>,
    /// Whether this is a picture, a movie, or a sound — which widget the
    /// frontend builds over [`rows_span`](MediaInfo::rows_span). A frontend that
    /// handles only some kinds leaves the rest as core's placeholder rows, which
    /// already read sensibly on their own.
    pub kind: MediaKind,
    /// The media's link destination — a path, URL, or `data:` URI, verbatim from
    /// the AST. A frontend resolves a relative path against the document's own
    /// directory; core does no I/O. For a `<picture>` this is the `<img>`
    /// fallback — the source used when no [`sources`](MediaInfo::sources) media
    /// query matches (or the frontend has no theme). Empty when a `<video>`/
    /// `<audio>` carries no `src` and names its candidates in `<source>`s
    /// instead; [`resolve`](MediaInfo::resolve) already accounts for that.
    pub destination: String,
    /// The `<source>` alternatives in document order, or empty for a plain
    /// image. See [`MediaSource`]; a theme- or codec-aware frontend picks one and
    /// otherwise loads [`destination`](MediaInfo::destination).
    pub sources: Vec<MediaSource>,
    /// The media's alt text, flattened from its inline children (empty when it
    /// has none).
    pub alt: String,
    /// A `<video poster="…">`'s still frame, or empty when there is none — an
    /// image destination, resolved exactly as [`destination`] is.
    ///
    /// [`destination`]: MediaInfo::destination
    pub poster: String,
}

/// One leaf directive (`::name{…}`) as a frontend sees it: which rows its
/// placeholder occupies, its type, and its attributes. A plain surface paints
/// the `⧉ name` placeholder glyphs as-is; a frontend that knows the host app's
/// vocabulary **skips the rows in `rows_span`** and paints the real thing there,
/// exactly as an image-capable one does with [`MediaInfo`]. Derived from
/// [`VRow::leaf_directive`] by [`directive_spans`].
///
/// Core resolves nothing here — it has no idea what an `embed` or a `toc` is,
/// and deliberately so: the directive vocabulary belongs to the app on top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectiveInfo {
    /// The [`VisualMap::rows`] rows this directive's placeholder occupies — the
    /// label row plus any blank fillers under it.
    pub rows_span: Range<usize>,
    /// The directive's type (`embed`, `toc`, `vis`), no leading colons.
    pub name: String,
    /// Its `{…}` attributes in source order; a bare one has a `None` value.
    pub attrs: Vec<(String, Option<String>)>,
    /// Its `[label]` text, flattened from its inline children (empty when it has
    /// none) — what the placeholder row shows.
    pub label: String,
}

impl DirectiveInfo {
    /// The value of attribute `key`, if it has one with a value. The convenience
    /// a frontend reaches for first (`info.attr("src")`), since almost every
    /// directive that draws as something real is pointed at by one attribute.
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_deref())
    }
}

impl MediaInfo {
    /// The image URL to load under `scheme`: the first [`sources`] `<source>`
    /// whose media query matches, else the [`destination`] `<img>` fallback. The
    /// pick is a `<source>`'s first `srcset` URL or the destination — a frontend
    /// resolves whichever it gets against the document directory exactly as it
    /// resolves `destination`, and reserves/keys the picture under `destination`
    /// regardless, so a theme switch just re-picks without disturbing the layout.
    ///
    /// Only `prefers-color-scheme` is understood (that's what a light/dark banner
    /// uses); a `<source>` with any other media query is skipped, and one with no
    /// media at all always matches (an unconditional override). With no matching
    /// source — including every frontend that can't/doesn't theme and passes
    /// [`ColorScheme::Light`] to a dark-only picture — it's the plain `<img>`.
    ///
    /// [`sources`]: MediaInfo::sources
    /// [`destination`]: MediaInfo::destination
    pub fn resolve(&self, scheme: ColorScheme) -> &str {
        if let Some(url) = self
            .sources
            .iter()
            .find(|s| media_matches(&s.media, scheme))
            .and_then(|s| first_srcset_url(&s.srcset))
        {
            return url;
        }
        // A `<video>`/`<audio>` may carry no `src` of its own, naming its
        // candidates only in child `<source>`s — none of which matched above,
        // because a codec-typed `<source>` has no media query and core judges no
        // MIME types. Falling through to an empty destination would hand the
        // frontend nothing to load, so take the first candidate URL instead and
        // let the frontend reject it if it can't decode it. An `<img>` never
        // reaches this: its `src` is the picture.
        if self.destination.is_empty() {
            if let Some(url) = self.sources.iter().find_map(|s| first_srcset_url(&s.srcset)) {
                return url;
            }
        }
        &self.destination
    }

    /// The **still picture** that stands for this media under `scheme`, for a
    /// frontend that can rasterize an image but not play a movie — a terminal, or
    /// a GUI still growing its player. `None` when there is no picture to draw,
    /// which is the honest answer for audio and for a poster-less video: the
    /// caller leaves core's labelled placeholder row, which already reads as
    /// *a thing that isn't text*.
    ///
    /// This exists so those frontends never hand a `.mp4` to an image decoder.
    /// That fails harmlessly today (a failed decode falls back to the same
    /// placeholder), but it spends a file read and a decode attempt per frame to
    /// arrive where this gets in one match.
    pub fn still(&self, scheme: ColorScheme) -> Option<&str> {
        match self.kind {
            MediaKind::Image => Some(self.resolve(scheme)),
            // A `poster` is an image destination, so it resolves the same way —
            // but it is named directly and has no `<source>` alternatives of its
            // own, so it needs no theme matching.
            MediaKind::Video if !self.poster.is_empty() => Some(&self.poster),
            MediaKind::Video | MediaKind::Audio => None,
        }
    }
}

/// A frontend's active color scheme — what a `<picture>`'s `prefers-color-scheme`
/// `<source>`s are matched against by [`MediaInfo::resolve`]. A frontend with no
/// notion of theme passes [`Light`](ColorScheme::Light), the web's own default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

/// Whether a `<source media="…">` query applies under `scheme`. Empty media is
/// an unconditional `<source>` (always matches); otherwise only a
/// `prefers-color-scheme: dark|light` feature is understood — anything else
/// (a width query, `print`, …) doesn't match, so resolution falls through to the
/// next source or the `<img>`. Deliberately lax about the surrounding syntax
/// (`(prefers-color-scheme: dark)`, `screen and (prefers-color-scheme:dark)`):
/// it keys off the feature and its value, which is all the theme case needs.
fn media_matches(media: &str, scheme: ColorScheme) -> bool {
    let media = media.trim();
    if media.is_empty() {
        return true;
    }
    let lower = media.to_ascii_lowercase();
    let Some(after) = lower.split_once("prefers-color-scheme").map(|(_, rest)| rest) else {
        return false;
    };
    // Skip the `:` and any spaces to reach the value word.
    let value = after.trim_start_matches([':', ' ', '\t']);
    let wanted = match scheme {
        ColorScheme::Light => "light",
        ColorScheme::Dark => "dark",
    };
    value.starts_with(wanted)
}

/// The first URL in a `srcset`: its first comma-separated candidate, before any
/// `1x`/`2x`/width descriptor. The theme case only ever puts one URL per
/// `<source>`, so the first candidate is the picture.
fn first_srcset_url(srcset: &str) -> Option<&str> {
    let first = srcset.split(',').next()?.trim();
    first.split_whitespace().next().filter(|u| !u.is_empty())
}

/// The narrowest a column may be squeezed. Below a few characters a column
/// stops carrying text and just shreds it one letter per line, which is worse
/// than letting the grid run wide.
const MIN_COL_WIDTH: usize = 3;

/// Shrink `widths` until the grid fits `avail` screen columns, taking from the
/// widest column each time so the loss is shared out rather than falling on
/// whichever column happens to be last. No column goes below
/// [`MIN_COL_WIDTH`]; a table with more columns than the surface has room for
/// still overflows, which is the honest outcome — there's nothing left to give.
fn fit_widths(widths: &mut [usize], avail: usize) {
    // Chrome: each column is its content plus a gutter either side, and every
    // column is closed by a `│` — with one more opening the row.
    let budget = avail.saturating_sub(3 * widths.len() + 1);
    while widths.iter().sum::<usize>() > budget {
        let Some(w) = widths.iter_mut().filter(|w| **w > MIN_COL_WIDTH).max() else {
            return;
        };
        *w -= 1;
    }
}

/// Word-wrap `glyphs` into lines of at most `width` columns, hard-breaking any
/// single word too long to fit.
///
/// Unlike a paragraph — where an overlong word just trails off the end of the
/// line — a table column is a hard boundary: a glyph past it lands on top of
/// the border, or on the next cell. So the width here is a promise, and a word
/// that won't keep it is broken.
///
/// The space at a break is dropped rather than hung past the edge. Its offset
/// isn't lost: the caller gives every line an end stop just past its last
/// glyph, which is exactly where that space was.
///
/// `width` is in display columns, and a break only ever falls between grapheme
/// clusters. Both matter to more than the picture: the caller anchors each
/// line's end stop just past its last glyph, so a line cut mid-cluster would
/// put a caret stop inside a character — reachable by Down or a click, and the
/// next Backspace would take the cluster apart from the middle.
///
/// An explicit in-cell break (a `\n` glyph, from a `<br>`) is a hard boundary:
/// each run between the breaks wraps on its own and the results stack. The break
/// glyphs are dropped — the caller's per-line end stop already sits exactly where
/// each break was, so no offset is lost.
fn wrap_glyphs(glyphs: &[Glyph], width: usize) -> Vec<Vec<Glyph>> {
    if glyphs.iter().any(|g| g.ch == '\n') {
        return glyphs
            .split(|g| g.ch == '\n')
            .flat_map(|seg| wrap_segment(seg, width))
            .collect();
    }
    wrap_segment(glyphs, width)
}

/// [`wrap_glyphs`] for a run with no explicit breaks — the word-wrap proper.
fn wrap_segment(glyphs: &[Glyph], width: usize) -> Vec<Vec<Glyph>> {
    let width = width.max(1);
    // Words are maximal non-space runs, each carrying the space that followed it
    // — which survives only if the next word joins it on this line.
    let mut words: Vec<(Vec<Glyph>, Option<Glyph>)> = Vec::new();
    let mut word: Vec<Glyph> = Vec::new();
    for g in glyphs {
        if g.ch == ' ' {
            words.push((std::mem::take(&mut word), Some(g.clone())));
        } else {
            word.push(g.clone());
        }
    }
    if !word.is_empty() {
        words.push((word, None));
    }

    let mut lines: Vec<Vec<Glyph>> = Vec::new();
    let mut line: Vec<Glyph> = Vec::new();
    let mut used = 0usize;
    let mut gap: Option<Glyph> = None;
    for (word, space) in words {
        for chunk in hard_break(&word, width) {
            let sep = gap.is_some() as usize;
            let cells = glyphs_width(chunk);
            if !line.is_empty() && used + sep + cells > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
                gap = None; // the break swallows the space
            }
            if let Some(sp) = gap.take() {
                line.push(sp);
                used += 1;
            }
            line.extend_from_slice(chunk);
            used += cells;
        }
        gap = space;
    }
    // An empty cell is still one (empty) line — it has an end the caret can
    // sit at, which is how you type into it.
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Break a single word into pieces of at most `width` columns, cutting only
/// between grapheme clusters — the replacement for slicing it into fixed runs
/// of glyphs, which measures a wide character as one column and can cut an
/// emoji in half.
///
/// A cluster wider than the whole column still gets a piece to itself: there is
/// nowhere legal to cut it, and overflowing by a cell is better than splitting a
/// character. An empty word yields no pieces at all, which is what keeps a
/// double space from opening a line of its own.
fn hard_break(word: &[Glyph], width: usize) -> Vec<&[Glyph]> {
    let mut out = Vec::new();
    if word.is_empty() {
        return out;
    }
    let (mut start, mut used) = (0usize, 0usize);
    for c in clusters(word) {
        if used > 0 && used + c.cells > width {
            out.push(&word[start..c.glyph]);
            start = c.glyph;
            used = 0;
        }
        used += c.cells;
    }
    out.push(&word[start..]);
    out
}

/// A table rule spanning `widths`, e.g. `┌──────┬─────┐`. Each column is its
/// content width plus the one-space gutter on either side.
fn rule_text(widths: &[usize], left: char, mid: char, right: char) -> String {
    let mut s = String::new();
    s.push(left);
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            s.push(mid);
        }
        for _ in 0..w + 2 {
            s.push('─');
        }
    }
    s.push(right);
    s
}

/// Push real document text: each glyph maps to its own source byte, and the one
/// that opens a grapheme cluster is the caret stop for the whole cluster.
///
/// Per cluster rather than per codepoint because a cluster is the character the
/// user sees, and it's the unit backspace and delete already step by. A stop
/// inside 👨‍👩‍👧 — five codepoints strung together with joiners — is a caret
/// parked in the middle of a character: one press of Right lands there, and the
/// next Backspace severs a joiner from what it joined, leaving a dangling ZWJ in
/// the source. The rest of the cluster still gets its glyph (it has to be
/// drawn); it just isn't somewhere to stand.
fn push_text(out: &mut Vec<Glyph>, text: &str, base_src: usize, style: Style) {
    for (gi, cluster) in text.grapheme_indices(true) {
        for (ci, ch) in cluster.char_indices() {
            out.push(Glyph { ch, style, src: base_src + gi + ci, stop: ci == 0 });
        }
    }
}

/// Emit an inline `str`/`smart_punctuation` run, mapping every visible char back
/// to its *true* source byte even when the source carries backslash escapes the
/// parsed `text` dropped (`\*` → `*`). The naive `span.start + text_offset`
/// mapping [`push_text`] uses drifts by one byte after each escape, so a caret or
/// click past an escaped `*` would land on the wrong character; walking the text
/// against its source keeps them aligned, and the hidden escape backslash gets no
/// glyph of its own (it is a spelling artefact, not something the caret lands on).
fn push_escaped_text(out: &mut Vec<Glyph>, text: &str, span: Range<usize>, source: &str, style: Style) {
    let end = span.end.min(source.len());
    let src = source.get(span.start..end).unwrap_or("");
    // Fast path — no dropped bytes, so text and source align 1:1 (the common
    // case: prose with no escapes). Byte lengths equal ⇒ no backslash was eaten.
    if src.len() == text.len() {
        push_text(out, text, span.start, style);
        return;
    }
    // Slow path: some `\` was consumed. Walk char-by-char, skipping a backslash
    // in the source exactly when it escapes the next visible char (a real escape),
    // never when it is a literal backslash the parse kept (that case has equal
    // lengths and takes the fast path above).
    let sb = src.as_bytes();
    let mut si = 0usize;
    for (_, cluster) in text.grapheme_indices(true) {
        for (ci, ch) in cluster.char_indices() {
            // Advance to the source character this one came from, stepping over
            // whatever the parse dropped on the way. An escape backslash is the
            // common case, but not the only one: a span can cover source that
            // was folded into a neighbouring node (smart punctuation next to a
            // bracket gives `text: "]"` over a source span of `"…]"`). Advancing
            // by the *text* character's length assumed escapes were the only
            // divergence, so one dropped multi-byte character desynchronized
            // every glyph after it — placing `]` inside the `…` before it.
            while si < sb.len() && src[si..].chars().next() != Some(ch) {
                si += src[si..].chars().next().map_or(1, char::len_utf8);
            }
            out.push(Glyph { ch, style, src: span.start + si.min(src.len()), stop: ci == 0 });
            si += src[si..].chars().next().map_or(ch.len_utf8(), char::len_utf8);
        }
    }
}

/// Build synthetic decoration glyphs (a bullet, a gutter) all pointing at `src`,
/// each carrying `role` so the frontend can style it (`Role::Body` for plain
/// padding). Synthetic glyphs are never caret stops — they share one offset, so
/// the caret steps over them (a click still lands at `src`).
fn synth(text: &str, role: Role, src: usize) -> Vec<Glyph> {
    let style = Style::default().role(role);
    text.chars()
        .map(|ch| Glyph { ch, style, src, stop: false })
        .collect()
}

fn concat(a: &[Glyph], b: &[Glyph]) -> Vec<Glyph> {
    let mut v = a.to_vec();
    v.extend_from_slice(b);
    v
}

/// The columns a row's prefix (a bullet, a quote gutter, an indent) takes up
/// before the text it introduces — what the wrap budget has left to spend.
fn prefix_width(prefix: &[Glyph]) -> usize {
    glyphs_width(prefix)
}

/// The label shown for an image with no alt text: the final path segment of its
/// destination (`img/cat.png` → `cat.png`), the whole destination when it has no
/// separator, and `"image"` when it's empty. A `data:` URI (which has no useful
/// tail) shows its scheme so the placeholder isn't a wall of base64.
fn media_label(dest: &str) -> String {
    if dest.is_empty() {
        return "image".to_string();
    }
    if dest.starts_with("data:") {
        return "data:…".to_string();
    }
    // Trim a query/fragment so a URL's `?v=2#frag` doesn't ride along.
    let clean = dest.split(['?', '#']).next().unwrap_or(dest);
    let tail = clean.trim_end_matches('/').rsplit(['/', '\\']).next().unwrap_or(clean);
    if tail.is_empty() { dest.to_string() } else { tail.to_string() }
}

/// A directive's attributes read as a human label — what a frontend puts on a
/// container's tinted panel, and what an attribute-bearing inline directive
/// shows in its chip.
///
/// Reads BOTH conventions diaryx content actually uses: twig's own dot-prefixed
/// classes (`{.public .family}`, arriving as one combined `class` attr) and bare
/// pandoc-style words with no leading dot (`{public family}` — what
/// `diaryx_core::visibility`'s publish-time filter and apps/web's directive
/// serializer both write, and which twig parses as one valueless attribute
/// each). Reading only `.class` would leave every *existing* diaryx `:::vis{…}`
/// block unlabeled. A `key=value` attr is configuration rather than a name, so
/// it contributes nothing. `None` when nothing readable is left.
fn directive_attr_label(attrs: &[(String, Option<String>)]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in attrs {
        if k == "class" {
            if let Some(v) = v {
                if !v.is_empty() {
                    parts.push(v.clone());
                }
            }
        } else if v.as_deref().unwrap_or("").is_empty() {
            parts.push(k.clone());
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn heading_style(level: u32) -> Style {
    // Just the role — a frontend decides how a heading of this level *looks*
    // (the terminal cycles a color and bolds it, the GUI scales the font). The
    // author wrote no emphasis here, so core records none. `level as u8` is safe:
    // Markdown/Djot cap headings at 6.
    Style::default().role(Role::Heading(level.min(255) as u8))
}

/// Is this `container` node a *directive* (`:::note{…}`, `::embed{…}`,
/// `:vis[…]`) rather than an HTML element (`<video>`, `<picture>`, `<div>`)?
///
/// twig 2.8 folded `div`/`span`/`directive`/`element` into one `container` kind,
/// and left nothing that separated them: `kind`, `name` and `directive_form` all
/// agree, field for field, on an HTML `<div>` and a Markdown `:::div`. Leaf
/// answered it by sniffing the span for whichever of `:` or `<` came first.
/// twig 3.0 records the answer at parse time as [`ContainerOrigin`], so this is
/// now the parser's own knowledge rather than a guess rebuilt from the bytes it
/// consumed.
pub(crate) fn container_is_directive(node: &FlatNode) -> bool {
    node.origin == Some(ContainerOrigin::Directive)
}

/// The tag a `container` node carries when it is an HTML element rather than a
/// directive — `Some("video")` for a promoted `<video>`, `None` for a `:::note`
/// or for any node that is not a container at all.
pub(crate) fn element_tag(node: &FlatNode) -> Option<&str> {
    (node.origin == Some(ContainerOrigin::Element))
        .then_some(node.name.as_deref())
        .flatten()
}

pub(crate) fn is_inline(node: &FlatNode) -> bool {
    // A directive is inline only in its `text` form (`:name[label]{…}`); the
    // `leaf` and `container` forms are blocks. All three report the same `kind`,
    // so the form is the only thing telling them apart — and getting it wrong
    // costs a whole paragraph: a text directive misread as a block makes its
    // paragraph fail the "all children inline" test in `block`, and the line is
    // then walked as a container of blocks, rendering as empty rows with no
    // caret home at all.
    //
    // An HTML element shares the `container` kind but never the `text` form, so
    // it answers `false` here and is walked as the block it is.
    if node.kind == Kind::Container {
        return container_is_directive(node) && node.directive_form == Some(DirectiveForm::Text);
    }
    is_inline_kind(&node.kind)
}

/// [`is_inline`] by kind alone — for the ancestor walks, whose `QueryMatch`es
/// carry no `directive_form`. It answers `false` for every directive, which its
/// callers must (and do) reconcile: they pair it with `is_block_container`,
/// which claims every directive, so the pair's verdict is the same one a form
/// would have given. Anything looking at a *directive itself* wants [`is_inline`]
/// and a real node.
pub(crate) fn is_inline_kind(kind: &Kind) -> bool {
    matches!(
        kind,
        Kind::Str
            | Kind::SoftBreak
            | Kind::HardBreak
            | Kind::NonBreakingSpace
            | Kind::Emph
            | Kind::Strong
            | Kind::Mark
            | Kind::Insert
            | Kind::Delete
            | Kind::Verbatim
            | Kind::InlineMath
            | Kind::DisplayMath
            | Kind::Url
            | Kind::Email
            | Kind::Link
            | Kind::Image
            | Kind::SmartPunctuation
            | Kind::Superscript
            | Kind::Subscript
            | Kind::FootnoteReference
    )
}

/// Assert two maps are identical down to every glyph, stop, and table span — the
/// contract `build_cached` and `build_spliced` must hold against `build`. Lives
/// at module scope (not in `mod tests`) so the Doc-driven differential test in
/// `doc.rs` can reach it and the private `stops` field it compares.
#[cfg(test)]
pub(crate) fn assert_maps_eq(a: &VisualMap, b: &VisualMap, ctx: &str) {
    assert_eq!(a.rows.len(), b.rows.len(), "row count ({ctx})");
    for (i, (ra, rb)) in a.rows.iter().zip(&b.rows).enumerate() {
        assert_eq!(ra.end_src, rb.end_src, "row {i} end_src ({ctx})");
        assert_eq!(ra.decoration, rb.decoration, "row {i} decoration ({ctx})");
        // The incremental walk labels a boundary from a query match's kind
        // string and the whole-arena walk from a `FlatNode`'s; this is what says
        // the two doors reach the same answer.
        assert_eq!(ra.boundary, rb.boundary, "row {i} boundary ({ctx})");
        assert_eq!(ra.code, rb.code, "row {i} code ({ctx})");
        assert_eq!(ra.code_lang, rb.code_lang, "row {i} code_lang ({ctx})");
        assert_eq!(ra.glyphs.len(), rb.glyphs.len(), "row {i} glyph count ({ctx})");
        for (j, (ga, gb)) in ra.glyphs.iter().zip(&rb.glyphs).enumerate() {
            assert_eq!(
                (ga.ch, ga.src, ga.stop, ga.style),
                (gb.ch, gb.src, gb.stop, gb.style),
                "row {i} glyph {j} ({ctx})"
            );
        }
    }
    assert_eq!(a.content_start, b.content_start, "content_start ({ctx})");
    assert_eq!(a.stops, b.stops, "stops ({ctx})");
    assert_eq!(a.tables.len(), b.tables.len(), "table count ({ctx})");
    for (i, (ta, tb)) in a.tables.iter().zip(&b.tables).enumerate() {
        assert_eq!(ta.rows_span, tb.rows_span, "table {i} rows_span ({ctx})");
        assert_eq!(ta.end_src, tb.end_src, "table {i} end_src ({ctx})");
    }
    assert_eq!(a.code_blocks, b.code_blocks, "code_blocks ({ctx})");
    assert_eq!(a.media, b.media, "images ({ctx})");
}

#[cfg(test)]
mod tests {
    use super::*;
    use twig::{Editor, Format, NodeId};

    fn map(src: &str) -> VisualMap {
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        build_t(&ed.nodes().unwrap(), src, Some(80))
    }

    /// [`map`] over a Djot source. Djot is the format that spells superscript
    /// and subscript at all — Markdown has no syntax for either.
    fn map_djot(src: &str) -> VisualMap {
        let mut ed = Editor::new_str(src, Format::Djot).unwrap();
        build_t(&ed.nodes().unwrap(), src, Some(80))
    }

    /// The baseline every glyph spelling `ch` was built with, in row order —
    /// how a test reads a raised or lowered run off the map without caring
    /// which row it landed on.
    fn baselines_of(m: &VisualMap, ch: char) -> Vec<Baseline> {
        m.rows
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .filter(|g| g.ch == ch)
            .map(|g| g.style.baseline)
            .collect()
    }

    /// [`map`] at a chosen wrap width.
    fn map_at(src: &str, wrap: Option<usize>) -> VisualMap {
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        build_t(&ed.nodes().unwrap(), src, wrap)
    }

    /// [`map`], but with twig's `directives` extension on (off by twig's own
    /// default) — the `:::name{.class}` fenced-div containers leaf-core's
    /// `"directive"` wysiwyg arm renders.
    fn map_directives(src: &str) -> VisualMap {
        let mut ed = Editor::new_ext(
            src.as_bytes(),
            Format::Markdown,
            twig::MarkdownExtensions { directives: true, ..Default::default() },
        )
        .unwrap();
        build_t(&ed.nodes().unwrap(), src, Some(80))
    }

    /// [`map`] with soft breaks preserved (`LineFlow::Preserve`).
    fn map_preserve(src: &str, wrap: Option<usize>) -> VisualMap {
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        build(&ed.nodes().unwrap(), src, wrap, true, &HashMap::new(), None)
    }

    /// The cache-free reference [`build`], with no per-image height overrides —
    /// every block image stays its default one-row placeholder. The tests that
    /// need a taller image drive it through [`crate::Doc::set_media_rows`] instead.
    fn build_t(nodes: &[FlatNode], src: &str, wrap: Option<usize>) -> VisualMap {
        build(nodes, src, wrap, false, &HashMap::new(), None)
    }

    fn rendered(m: &VisualMap) -> String {
        m.rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render a source both ways: `build` over the whole marshalled arena (the
    /// reference), and `build_cached` driven the way [`crate::Doc`] drives it —
    /// top-level blocks from `child_spans`, per-block subtrees on a miss.
    fn render_both(
        ed: &mut Editor,
        src: &str,
        wrap: Option<usize>,
        cache: &mut BlockCache,
    ) -> (VisualMap, VisualMap) {
        let all = ed.nodes().unwrap();
        let media_rows = HashMap::new();
        let plain = build(&all, src, wrap, false, &media_rows, None);
        let top = top_blocks(ed);
        let cached = build_cached(&top, src, wrap, false, &media_rows, None, cache, |id| {
            ed.subtree(NodeId(id)).unwrap_or_default()
        });
        (plain, cached)
    }

    /// The whole correctness claim of the block cache: `build_cached` produces a
    /// byte-identical map to `build`, on a fresh cache *and* — the case that
    /// actually exercises reuse-and-shift plus per-block subtree marshalling — on
    /// a warm cache after the source has been edited underneath it.
    /// **Every glyph must stand on the character it claims.** A row's source
    /// extent is computed from its last glyph's offset, so a glyph carrying an
    /// offset that is not its own character's start yields a row end inside a
    /// multi-byte character — and every later slice of the source panics on it.
    ///
    /// Reproduces a real crash from a journal entry: a bracketed elision inside
    /// a blockquote (`[…]`) gave the closing bracket a `text` of `"]"` over a
    /// source span covering `"…]"`, because the parse folded the ellipsis into a
    /// neighbouring node. `push_escaped_text` walked that span assuming a
    /// dropped backslash was the only way text and source could diverge, so the
    /// `]` landed on the `…`'s first byte:
    /// `byte index 1236 is not a char boundary; it is inside '…'`.
    #[test]
    fn a_glyph_never_lands_inside_the_character_before_it() {
        let src = "> engage with it rather than look away. […]\n>\n> The through-line\n";
        let vmap = map(src);
        for (r, row) in vmap.rows.iter().enumerate() {
            assert!(
                src.is_char_boundary(row.end_src.min(src.len())),
                "row {r} ends at {} — inside a character",
                row.end_src
            );
            for g in &row.glyphs {
                assert!(
                    src.is_char_boundary(g.src.min(src.len())),
                    "row {r} has {:?} at {}, which is inside a character",
                    g.ch,
                    g.src
                );
            }
        }
        // The elision survives, and its bracket sits on the real `]`.
        let text: String = vmap.rows.iter().flat_map(|r| r.glyphs.iter().map(|g| g.ch)).collect();
        assert!(text.contains("[…]"), "the elision should render: {text:?}");
        let close = vmap
            .rows
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .find(|g| g.ch == ']')
            .expect("a closing bracket");
        assert_eq!(
            src[close.src..].chars().next(),
            Some(']'),
            "the bracket glyph should stand on the source's own `]`"
        );
    }

    #[test]
    fn build_cached_matches_build() {
        let docs = [
            "# Title\n\nThe quick brown fox.\n\nAnother paragraph here.\n",
            "## H\n\n- one\n- two\n- three\n\n> a quote\n> continued\n",
            "para one\n\n```\ncode\nlines\n```\n\nafter code\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n\ntext after a table\n",
            "line\n- \nsetext?\n\nreal para\n\n\n\ntrailing blanks\n",
            "> quote with **bold** and a [link](https://x.dev)\n>\n> - item\n> - item2\n\ntail\n",
            "intro\n\n![a cat](img/cat.png)\n\nbetween\n\n![](https://x.dev/logo.svg)\n\nend\n",
            "- text item\n- ![alt](pic.png)\n- more text\n",
            // Footnotes: twig parses each definition as a root beside `doc`, so
            // these are the docs where the reference build and the incremental
            // one could disagree about what the top-level blocks even are.
            "A claim[^1] and another[^src].\n\n[^1]: First note.\n\n[^src]: Second.\n\ntail\n",
            "note[^a]\n\n[^a]: body **bold**\n    wrapped on\n    three lines\n\nafter\n",
            // No trailing newline. twig closes the document's last block on the
            // virtual newline it supplies at EOF, so that block's `span.end` is
            // `source.len() + 1` — a range that slices no bytes at all. Keying
            // the block cache off such a slice made every last block hash alike;
            // see [`block_bytes`].
            "# Title\n\nThe quick brown fox.\n\nA tail with no newline",
            "A claim[^1] and another[^src].\n\n[^1]: First note.\n[^src]: Second, ending the file.",
        ];
        for wrap in [None, Some(80usize), Some(20)] {
            for src in docs {
                let ctx = format!("wrap={wrap:?} src={src:?}");
                let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
                let mut cache = BlockCache::default();

                // 1) Fresh cache equals the cache-free build.
                let (plain, cached) = render_both(&mut ed, src, wrap, &mut cache);
                assert_maps_eq(&plain, &cached, &format!("fresh {ctx}"));

                // 2) Type a char mid-document, reparse, rebuild with the now-warm
                //    cache: the edited block is re-marshalled and re-rendered,
                //    every block below it is reused shifted, and the result must
                //    still match a from-scratch build.
                let at = (src.len() / 2..=src.len())
                    .find(|&i| src.is_char_boundary(i))
                    .unwrap();
                ed.edit_range(at, at, "Z").unwrap();
                let src2 = ed.source_str().unwrap();
                let (plain2, cached2) = render_both(&mut ed, &src2, wrap, &mut cache);
                assert_maps_eq(&plain2, &cached2, &format!("after insert {ctx}"));

                // 3) Delete it again: offsets shift back the other way, and the
                //    warm cache must not hand back stale shifted rows.
                ed.edit_range(at, at + 1, "").unwrap();
                let src3 = ed.source_str().unwrap();
                let (plain3, cached3) = render_both(&mut ed, &src3, wrap, &mut cache);
                assert_maps_eq(&plain3, &cached3, &format!("after delete {ctx}"));
            }
        }
    }

    /// A document that does not end in a newline is the one place twig hands
    /// leaf a top-level span that addresses no source: the last block is closed
    /// on the virtual newline the parser supplies at EOF, so its `span.end` is
    /// `source.len() + 1`. The block cache keys on the bytes under that span, and
    /// reading the out-of-range slice as *no bytes* broke it two ways at once —
    /// [`block_bytes`] has the full account. Both ways are checked here, because
    /// they fail independently.
    #[test]
    fn a_block_running_past_the_last_byte_still_keys_the_cache_by_its_own_bytes() {
        // One: two overrunning blocks collide. A footnote definition is a root
        // beside `doc` that [`top_blocks`] merges into the top level, while the
        // `section` above it spans the definition's bytes too — so when the
        // definition ends the file, both blocks end past it. The second was
        // served the first's rows, and the definition rendered as a copy of the
        // heading.
        let src = "A claim[^1] worth checking.\n\n# A heading with a reference[^1] in it\n\n[^1]: The first note.\n[^note]: A note with a word for a label.";
        let mut ed = Editor::new_str(src, Format::Djot).unwrap();
        let (plain, cached) = render_both(&mut ed, src, Some(80), &mut BlockCache::default());
        assert_maps_eq(&plain, &cached, "a definition ending the file");
        let text = rendered(&cached);
        assert!(
            text.ends_with("[note] A note with a word for a label."),
            "the last definition should render itself: {text:?}"
        );
        assert_eq!(
            text.matches("A heading with a reference").count(),
            1,
            "the heading should render exactly once: {text:?}"
        );

        // Two: one overrunning block goes stale. Its bytes are its cache key, so
        // a block that keeps hashing the same however it is edited is served the
        // rows built before the edit — the whole last line frozen as the user
        // types in it.
        let mut cache = BlockCache::default();
        let first = "first para\n\n# A heading\n\nlast para with no newline";
        let mut ed = Editor::new_str(first, Format::Djot).unwrap();
        let (_, warm) = render_both(&mut ed, first, Some(80), &mut cache);
        assert!(rendered(&warm).ends_with("last para with no newline"));

        let second = "first para\n\n# A heading\n\nDIFFERENT text without a newline";
        let mut ed = Editor::new_str(second, Format::Djot).unwrap();
        let (plain, cached) = render_both(&mut ed, second, Some(80), &mut cache);
        assert_maps_eq(&plain, &cached, "edited last block, warm cache");
        let text = rendered(&cached);
        assert!(
            text.ends_with("DIFFERENT text without a newline"),
            "the warm cache served the pre-edit rows: {text:?}"
        );
    }

    #[test]
    fn resolves_markup_to_plain_text() {
        let text = rendered(&map("# Title\n\na **bold** word\n"));
        assert!(!text.contains('#'), "heading marker shown: {text:?}");
        assert!(!text.contains("**"), "strong delimiters shown: {text:?}");
        assert!(text.contains("Title") && text.contains("bold word"));
    }

    #[test]
    fn every_glyph_points_at_its_source_byte() {
        let src = "a **bold** c\n";
        let m = map(src);
        for row in &m.rows {
            for g in &row.glyphs {
                // A real (non-synthetic) glyph's source byte is the glyph's char.
                if g.src < src.len() && src.is_char_boundary(g.src) {
                    if let Some(sc) = src[g.src..].chars().next() {
                        if sc == g.ch {
                            continue;
                        }
                    }
                }
                // Synthetic prefixes (none here) would be the only exceptions.
                panic!("glyph {:?} at src {} doesn't match source", g.ch, g.src);
            }
        }
    }

    #[test]
    fn offset_and_position_round_trip_on_visible_text() {
        let m = map("hello world\n");
        let (r, c) = m.pos_of_offset(6); // the 'w'
        assert_eq!(m.offset_of_pos(r, c), 6);
    }

    #[test]
    fn unwrapped_mode_emits_one_row_per_paragraph() {
        // A long paragraph that would wrap under a column budget stays a single
        // row when wrap is None (the GUI wraps it at pixel width instead).
        let long = "one two three four five six seven eight nine ten eleven twelve\n";
        let mut ed = Editor::new_str(long, Format::Markdown).unwrap();
        let wrapped = build_t(&ed.nodes().unwrap(), long, Some(12));
        let unwrapped = build_t(&ed.nodes().unwrap(), long, None);
        assert!(wrapped.num_rows() > 1, "narrow column should wrap");
        assert_eq!(unwrapped.num_rows(), 1, "no budget should keep it one row");
        // Every glyph's source byte is preserved in the single row.
        let text: String = unwrapped.rows[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(text.trim_end(), long.trim_end());
    }

    fn line_texts(m: &VisualMap) -> Vec<String> {
        m.rows
            .iter()
            .map(|r| {
                // Trim the trailing whitespace a row may carry — the zero-width
                // '\n' that closes a preserved line, and any space glyph left at
                // a wrap boundary (both real caret stops, neither visible text).
                r.glyphs.iter().map(|g| g.ch).collect::<String>().trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn preserve_lays_each_soft_break_on_its_own_row() {
        // A soft break (a bare newline inside a paragraph) folds into a space by
        // default — the whole paragraph is one reflowed row...
        let src = "one two\nthree four\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let folded = build_t(&ed.nodes().unwrap(), src, None);
        assert_eq!(folded.num_rows(), 1, "fold: one reflowed row");
        assert_eq!(line_texts(&folded), vec!["one two three four"], "break folded to a space");

        // ...and under Preserve it renders where it was written, a row per line.
        let kept = map_preserve(src, None);
        assert_eq!(line_texts(&kept), vec!["one two", "three four"], "preserve: a row per line");
    }

    #[test]
    fn a_preserved_break_keeps_the_newline_offset_as_a_caret_stop() {
        // The break must leave a caret stop at the newline byte, or the caret
        // could not rest at the end of the first line. The '\n' glyph is dropped
        // from the row (so nothing stray renders); its offset (7 here) becomes the
        // row's end stop instead — the same offset the folded space would carry.
        let src = "one two\nthree four\n";
        let m = map_preserve(src, None);
        assert!(!m.rows[0].glyphs.iter().any(|g| g.ch == '\n'), "the break glyph is dropped");
        assert_eq!(m.rows[0].end_src, 7, "the first row ends at the newline byte");
        assert!(m.is_stop(7), "the newline offset is a caret stop");
        // Row end offsets stay strictly ascending — no two rows pin one offset.
        let offs: Vec<usize> = m.rows.iter().map(|r| r.end_src).collect();
        assert!(offs.windows(2).all(|w| w[0] < w[1]), "offsets not unique: {offs:?}");
    }

    #[test]
    fn preserved_lines_wrap_independently() {
        // Each preserved line wraps to the column on its own; the break between
        // them is hard, so a word never crosses it — "gamma" and "delta" could
        // share a row on width alone but the soft break keeps them apart.
        let src = "alpha beta gamma\ndelta epsilon\n";
        let m = map_preserve(src, Some(12));
        assert_eq!(
            line_texts(&m),
            vec!["alpha beta", "gamma", "delta", "epsilon"],
            "each source line wraps on its own"
        );
    }

    #[test]
    fn an_empty_paragraph_between_blocks_renders_its_own_rows() {
        // "A", then two blank lines (an empty paragraph opened with Enter), then
        // "B": the empty paragraph must be navigable rows, not collapsed onto B.
        // Rows: "A", spacer, empty-paragraph, spacer, "B" — each blank row a
        // distinct source offset.
        let m = map("A\n\n\n\nB\n");
        let text: Vec<String> = m
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect();
        assert_eq!(text, vec!["A", "", "", "", "B"], "got {text:?}");
        let offs: Vec<usize> = m.rows.iter().map(|r| r.end_src).collect();
        // Strictly ascending — no two rows share an offset (else the caret pins).
        assert!(offs.windows(2).all(|w| w[0] < w[1]), "offsets not unique: {offs:?}");
    }

    #[test]
    fn a_tight_block_boundary_still_gets_one_separator() {
        // A heading directly above text (no blank line between) keeps the single
        // conventional separator row, as before.
        let m = map("# H\ntext\n");
        let text: Vec<String> = m
            .rows
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect();
        assert_eq!(text, vec!["H", "", "text"], "got {text:?}");
    }

    #[test]
    fn an_escaped_delimiter_renders_without_its_backslash_and_maps_true_offsets() {
        // `a\*b` renders the three visible chars `a * b` — the escape backslash
        // is hidden — and every glyph points at its real source byte, so a caret
        // past the escape lands right (the `*` at source 2, `b` at source 3, not
        // the drifted 1/2 the naive text-offset mapping gave).
        let m = map("a\\*b\n");
        let row: Vec<(char, usize)> = m.rows[0].glyphs.iter().map(|g| (g.ch, g.src)).collect();
        assert_eq!(row, vec![('a', 0), ('*', 2), ('b', 3)], "got {row:?}");
    }

    #[test]
    fn an_escaped_hash_stays_a_paragraph_and_shows_the_hash() {
        // `\# hi` is a paragraph beginning with a literal `#`, not a heading —
        // the backslash is hidden, the `#` shown at its true offset.
        let m = map("\\# hi\n");
        let text: String = m.rows[0].glyphs.iter().map(|g| g.ch).collect();
        assert_eq!(text, "# hi");
        assert_eq!(m.rows[0].glyphs[0].src, 1, "the # is at source byte 1, past the \\");
    }

    #[test]
    fn a_tight_nested_list_hangs_its_sublist_directly_under_the_item() {
        // A list item's own text and the sub-list nested under it are written on
        // adjacent source lines, so the rich view butts them together — no
        // fabricated blank row. Regression: the synthetic "breathe" separator
        // used to open a gap between `• a` and its `  • b`.
        assert_eq!(rendered(&map("- a\n  - b\n")), "• a\n  • b");
    }

    #[test]
    fn a_loose_nested_list_keeps_its_real_blank_line() {
        // A genuine blank source line (a loose list) still parts the item from
        // its sub-list — only the *fabricated* separator is suppressed, never a
        // real one the author typed. The gap row wears the item's continuation
        // prefix (the two-space indent), so it renders as "  ", not empty.
        assert_eq!(rendered(&map("- a\n\n  - b\n")), "• a\n  \n  • b");
    }

    #[test]
    fn frontmatter_is_hidden_and_the_document_opens_into_its_content() {
        // Leading YAML frontmatter renders nothing — no phantom blank rows for
        // its lines, no leading gap — and `content_start` points at the first
        // real block so the caret floor can keep out of the hidden metadata.
        let fm = "---\nconfig: prov.yaml\ncontents:\n- '[Sample](sample.md)'\n---\n";
        let src = format!("{fm}# leaf\n\nA line.\n");
        let m = map(&src);
        let text = rendered(&m);
        assert!(!text.contains("config"), "frontmatter body leaked: {text:?}");
        assert!(!text.contains("prov"), "frontmatter body leaked: {text:?}");
        assert_eq!(m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(), "leaf");
        assert_eq!(m.content_start, fm.len(), "floor should be the first real block");
    }

    #[test]
    fn a_document_without_frontmatter_has_a_zero_floor() {
        let m = map("# leaf\n\nbody\n");
        assert_eq!(m.content_start, 0);
    }

    #[test]
    fn trailing_spaces_become_caret_stops_so_the_caret_can_be_drawn_past_them() {
        // Markdown/Djot drop the trailing space in `hello ` from the `str` node,
        // so without help the row would end at `hello` and the caret couldn't be
        // drawn past column 5 — typing a space at a line's end wouldn't move it
        // on screen until the next visible character reparsed the space into an
        // interior node. The builder recovers it from the block's span/content_span
        // gap and emits it as a real, caret-stoppable glyph.
        let m = map("hello \n");
        assert_eq!(m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(), "hello ");
        assert_eq!(m.rows[0].end_src, 6, "the row now ends past the trailing space");
        // The caret can rest both on and past the space.
        assert_eq!(m.pos_of_offset(5), (0, 5), "between 'o' and the space");
        assert_eq!(m.pos_of_offset(6), (0, 6), "past the space");
        // Two trailing spaces, both stops.
        let m = map("hello  \n");
        assert_eq!(m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(), "hello  ");
        assert_eq!(m.pos_of_offset(7), (0, 7));
    }

    #[test]
    fn a_headings_trailing_space_is_a_caret_stop_too() {
        // The hidden `# ` marker means `# hi ` renders as `hi ` in three columns;
        // the caret past the trailing space lands on the third.
        let m = map("# hi \n");
        assert_eq!(m.rows[0].glyphs.iter().map(|g| g.ch).collect::<String>(), "hi ");
        assert_eq!(m.pos_of_offset(5), (0, 3));
    }

    #[test]
    fn a_table_cells_trailing_padding_is_not_mistaken_for_block_trailing_space() {
        // A cell's own `span` is the whole row, so the trailing-whitespace
        // recovery must not run for cells or it would swallow the `│` delimiters
        // and neighbours between the cell text and the row's end. The grid stays
        // exactly as before.
        let text = rendered(&map(TABLE));
        assert!(text.contains("│ Pear │   3 │"), "cell padding disturbed:\n{text}");
    }

    #[test]
    fn a_click_below_the_last_row_lands_on_the_last_stop_not_offset_zero() {
        // A drag into the empty space under a short document used to resolve to
        // offset 0 — the wrong direction, and not even a caret stop when the
        // document opens on hidden frontmatter (its `content_start` floor is not
        // a stop), which crashed the caret invariant. It now lands on the last
        // stop: the end of the document, where dragging downward should reach.
        let fm = "---\ntitle: n\n---\n";
        let m = map(&format!("{fm}# Hi\n\nbody\n"));
        let below = m.num_rows() + 5;
        let off = m.offset_of_pos(below, 0);
        assert!(m.is_stop(off), "offset {off} from a below-content click is not a stop");
        assert_eq!(off, m.stops.last().copied().unwrap(), "should be the document's last stop");
        assert!(off > fm.len(), "must not fall onto the hidden frontmatter floor");
    }

    #[test]
    fn offset_of_pos_is_a_stop_for_every_row_including_past_the_end() {
        // The invariant the caret motion asserts: whatever cell a click names,
        // the offset it resolves to is one the caret can actually rest at.
        for src in [
            "hello \n",
            "# A heading here \n\nbody text goes on \n",
            "---\nk: v\n---\n# Title\n\nprose here that wraps a bit \n",
        ] {
            let m = map(src);
            for row in 0..m.num_rows() + 3 {
                for col in 0..30 {
                    let off = m.offset_of_pos(row, col);
                    assert!(m.is_stop(off), "row {row} col {col} → {off} is not a stop in {src:?}");
                }
            }
        }
    }

    /// `| Name | Qty |` with Name left-aligned and Qty right-aligned.
    const TABLE: &str = "| Name | Qty |\n|:-----|----:|\n| Pear | 3 |\n| Fig | 12 |\n";

    #[test]
    fn a_table_renders_as_an_aligned_grid() {
        let text = rendered(&map(TABLE));
        assert_eq!(
            text,
            "┌──────┬─────┐\n\
             │ Name │ Qty │\n\
             ├──────┼─────┤\n\
             │ Pear │   3 │\n\
             │ Fig  │  12 │\n\
             └──────┴─────┘",
            "got:\n{text}"
        );
    }

    #[test]
    fn table_columns_honour_their_alignment() {
        // Centre and default(left) come straight from twig's cell.alignment —
        // the delimiter row it's spelled in is consumed and has no node.
        let text = rendered(&map("| A | Bee |\n| --- | :---: |\n| x | y |\n"));
        assert!(text.contains("│ x │  y  │"), "centred column: {text:?}");
    }

    #[test]
    fn table_borders_are_decoration_the_caret_never_lands_on() {
        let m = map(TABLE);
        // The rules are whole decoration rows.
        for r in [0, 2, 5] {
            assert!(m.rows[r].decoration, "row {r} should be a decoration rule");
            assert!(!m.rows[r].glyphs.iter().any(|g| g.stop), "row {r} has a stop");
        }
        // A content row's `│` and padding are decoration; only the cell text
        // and each cell's one end-stop are stops.
        let header = &m.rows[1];
        assert!(!header.decoration);
        for g in &header.glyphs {
            if g.ch == '│' {
                assert!(!g.stop, "a border is not a caret stop");
            }
        }
        let stops: String = header.glyphs.iter().filter(|g| g.stop).map(|g| g.ch).collect();
        assert_eq!(stops, "Name Qty ", "cell text plus one end-stop space each");
    }

    #[test]
    fn a_cell_maps_to_its_own_source_text() {
        let m = map(TABLE);
        // "Pear" starts at byte 32 in TABLE; the caret there draws on the 'P'.
        let pear = TABLE.find("Pear").unwrap();
        let (r, c) = m.pos_of_offset(pear);
        assert_eq!(m.rows[r].glyphs[c].ch, 'P');
        assert_eq!(m.offset_of_pos(r, c), pear, "round trips");
    }

    #[test]
    fn a_wide_table_is_cut_to_fit_and_its_cells_wrap() {
        // Columns wider than the surface used to run off the right edge, where
        // nothing could reach them. They're cut to the budget instead, and the
        // text wraps down inside the column — the header rule stays put, and
        // an alignment holds on every line of a wrapped cell, not just the first.
        let src = "| Ingredient | Notes |\n|---|---:|\n\
                   | flour milled coarse | sift it twice |\n| salt | a pinch |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build_t(&ed.nodes().unwrap(), src, Some(30));
        let text = rendered(&m);
        assert_eq!(
            text,
            "┌──────────────┬─────────────┐\n\
             │ Ingredient   │       Notes │\n\
             ├──────────────┼─────────────┤\n\
             │ flour milled │     sift it │\n\
             │ coarse       │       twice │\n\
             │ salt         │     a pinch │\n\
             └──────────────┴─────────────┘",
            "got:\n{text}"
        );
        for (r, row) in m.rows.iter().enumerate() {
            assert!(row.glyphs.len() <= 30, "row {r} overflows: {}", row.glyphs.len());
        }
    }

    #[test]
    fn a_column_too_narrow_for_a_word_breaks_it_rather_than_spilling() {
        // A paragraph lets an overlong word trail off the end of the line; a
        // table column can't — a glyph past the border lands on the border.
        let src = "| A | B |\n|---|---|\n| antidisestablishmentarianism | x |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build_t(&ed.nodes().unwrap(), src, Some(20));
        for (r, row) in m.rows.iter().enumerate() {
            assert!(row.glyphs.len() <= 20, "row {r} overflows: {}", row.glyphs.len());
        }
        // Broken across lines, but whole: every letter is still drawn, at its
        // own source byte, where the caret can reach it.
        let word = "antidisestablishmentarianism";
        let at = src.find(word).unwrap();
        for (i, ch) in word.char_indices() {
            assert!(
                m.rows
                    .iter()
                    .flat_map(|r| r.glyphs.iter())
                    .any(|g| g.stop && g.src == at + i && g.ch == ch),
                "{ch:?} at {} was lost to the break", at + i
            );
        }
    }

    #[test]
    fn a_code_block_maps_each_line_to_its_own_source_text() {
        // Every glyph used to point at the block's start, which made the whole
        // block one offset — visible, but impossible to put a caret inside.
        let src = "```rust\nlet x = 1;\nfn f() {}\n```\n";
        let m = map(src);
        for row in &m.rows {
            for g in row.glyphs.iter().filter(|g| g.stop) {
                assert_eq!(
                    src[g.src..].chars().next(),
                    Some(g.ch),
                    "glyph {:?} at {} isn't the source byte it claims",
                    g.ch,
                    g.src
                );
            }
        }
    }

    #[test]
    fn an_indented_code_block_maps_past_its_stripped_indent() {
        // twig strips the four-space indent, so `text` isn't a source slice and
        // the lines have to be re-found. Offsets land on the code, not the indent.
        let src = "    indented\n    code\n";
        let m = map(src);
        let stops: Vec<(char, usize)> = m
            .rows
            .iter()
            .flat_map(|r| r.glyphs.iter().filter(|g| g.stop).map(|g| (g.ch, g.src)))
            .collect();
        assert_eq!(stops[0], ('i', 4), "first line should start past the indent");
        assert!(stops.contains(&('c', 17)), "second line misplaced: {stops:?}");
    }

    #[test]
    fn a_fenced_block_whose_code_echoes_its_info_string_maps_to_the_code() {
        // The one case that defeats a forward search: the opening fence
        // ```` ```rust ```` ends with the same text as the code under it.
        let src = "```rust\nrust\n```\n";
        let m = map(src);
        let first = m.rows[0].glyphs.iter().find(|g| g.stop).unwrap();
        assert_eq!(first.src, 8, "matched the info string, not the code");
    }

    #[test]
    fn a_code_block_carries_no_gutter_and_is_published_as_a_row_span() {
        // The old `▏ ` gutter is gone: a code row is the block prefix (none, at
        // the top level) plus the code text, and the whole run is named in
        // `code_blocks` so a frontend can box it.
        let src = "para\n\n```\ncode\nlines\n```\n\nafter\n";
        let m = map(src);
        assert_eq!(m.code_blocks.len(), 1, "one code block");
        let span = m.code_blocks[0].rows_span.clone();
        let rows: Vec<String> = m.rows[span.clone()]
            .iter()
            .map(|r| r.glyphs.iter().map(|g| g.ch).collect())
            .collect();
        assert_eq!(rows, vec!["code".to_string(), "lines".to_string()]);
        assert!(!rendered(&m).contains('▏'), "gutter still drawn");
        assert!(
            m.rows[span].iter().all(|r| r.code),
            "every row in the span is flagged code"
        );
    }

    #[test]
    fn a_directive_container_is_tinted_and_labeled_on_its_first_row() {
        // diaryx's `:::vis{.public .family}` visibility block, and any other
        // `:::name{.class}` fenced div — core is agnostic of `name`.
        let src = ":::vis{.public .family}\nhello\n\nworld\n:::\nafter\n";
        let m = map_directives(src);

        let content_rows: Vec<usize> = (0..m.rows.len())
            .filter(|&i| m.rows[i].directive)
            .collect();
        assert!(!content_rows.is_empty(), "some row is flagged directive");

        let after_rows: Vec<usize> = (0..m.rows.len())
            .filter(|&i| !content_rows.contains(&i) && !m.rows[i].glyphs.is_empty())
            .collect();
        assert!(
            after_rows.iter().all(|&i| !m.rows[i].directive),
            "content outside the fence isn't tinted"
        );

        let labels: Vec<&str> = content_rows
            .iter()
            .filter_map(|&i| m.rows[i].directive_label.as_deref())
            .collect();
        assert_eq!(labels, vec!["public family"], "only the first row carries the label");

        assert_eq!(
            rendered(&m).lines().filter(|l| !l.is_empty()).collect::<Vec<_>>(),
            vec!["hello", "world", "after"],
            "fence markers don't leak into the rendered text"
        );
    }

    #[test]
    fn a_bare_word_directive_is_labeled_same_as_dot_classes() {
        // diaryx_core::visibility's own `:::vis{public family}` — no leading
        // dots — is what apps/web's directive serializer and the native
        // publish-time filter both actually write today, distinct from twig's
        // `.class` convention. Both must label the same way so every existing
        // diaryx `:::vis{...}` block reads, not just newly dot-authored ones.
        let src = ":::vis{public family}\nhello\n:::\n";
        let m = map_directives(src);
        let label = m.rows.iter().find_map(|r| r.directive_label.clone());
        assert_eq!(label.as_deref(), Some("public family"));
    }

    #[test]
    fn a_text_directive_keeps_its_paragraph_visible() {
        // Regression: an inline `:name[label]{…}` used to make its paragraph
        // fail the "all children inline" test, so the whole line was walked as
        // a container of blocks and rendered as empty rows with NO caret stops —
        // the text vanished from the editor and the caret couldn't enter it.
        // diaryx's inline `:vis[…]` is exactly this shape.
        let src = "Text with :abbr[HTML]{title=\"HyperText\"} inline.\n";
        let m = map_directives(src);
        assert_eq!(rendered(&m).trim_end(), "Text with HTML inline.");
        // Every character of the line is a caret home, markup excluded — the
        // label reads as ordinary text, the way a link's does.
        let stops: usize = m.rows.iter().map(|r| r.glyphs.iter().filter(|g| g.stop).count()).sum();
        assert_eq!(stops, "Text with HTML inline.".chars().count());
        // It is inline, so it is not the container form's tinted panel.
        assert!(m.rows.iter().all(|r| !r.directive));
    }

    #[test]
    fn a_text_directives_label_maps_to_its_true_source_bytes() {
        // Regression (needs twig-doc >= 2.5.0): twig parses a `[label]` as a
        // detached slice, and until it rebased the enclosing scan's segments
        // onto it every node inside the label reported a span of `(0,0)`. Read
        // by anything that trusts a span that means "byte 0", so the label's
        // glyphs mapped to the START OF THE DOCUMENT — a click on the label put
        // the caret at the top of the file, its stops collided with the real
        // first line's, and an edit there landed on the wrong bytes entirely.
        //
        // The sibling test `a_text_directive_keeps_its_paragraph_visible` only
        // counts stops, which is exactly why this went unnoticed: the right
        // NUMBER of stops at completely wrong offsets.
        let src = "x :abbr[HTML]{title=\"y\"} z\n";
        let m = map_directives(src);
        let stops: Vec<(char, usize)> = m
            .rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .filter(|g| g.stop)
            .map(|g| (g.ch, g.src))
            .collect();
        // `HTML` sits at 8..12. The name, brackets and `{…}` are hidden markup
        // the caret steps over, so the line's stops run 0, 1, 8..12, then 24.
        assert_eq!(
            stops,
            [('x', 0), (' ', 1), ('H', 8), ('T', 9), ('M', 10), ('L', 11), (' ', 24), ('z', 25)]
        );
    }

    #[test]
    fn every_glyph_in_a_directive_label_points_at_its_source_byte() {
        // The `every_glyph_points_at_its_source_byte` invariant, extended over
        // directive labels now that their offsets are real. Nested markup is
        // included: its delimiters are hidden, so the visible glyphs must skip
        // them and still name their own bytes.
        let src = "x :abbr[a *b* c] y and :vis[family only] z\n";
        let m = map_directives(src);
        for g in m.rows.iter().flat_map(|r| &r.glyphs).filter(|g| g.stop) {
            let at = src[g.src..].chars().next();
            assert_eq!(at, Some(g.ch), "glyph {:?} claims byte {}, which is {at:?}", g.ch, g.src);
        }
        assert_eq!(rendered(&m).trim_end(), "x a b c y and family only z");
    }

    #[test]
    fn a_directive_labels_nested_emphasis_keeps_both_its_style_and_its_offsets() {
        let src = "x :abbr[a *b* c] y\n";
        let m = map_directives(src);
        let b = m
            .rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .find(|g| g.ch == 'b')
            .expect("the emphasised char");
        assert!(b.style.italic, "the label's *b* lost its emphasis");
        assert_eq!(b.src, 11, "the label's *b* lost its source byte");
    }

    #[test]
    fn a_bare_colon_word_renders_as_the_prose_it_almost_always_is() {
        // Regression: twig matches a colon followed by any letter-led word, so
        // ordinary prose is full of "text directives" nobody meant to write.
        // With no `[label]` there are no children, and the arm recursed into
        // them — rendering *nothing*. The word vanished from the document with
        // no caret stop left behind, so it could not even be deleted.
        for src in ["a :word b\n", "note :see below\n", ":smile: hi\n"] {
            let m = map_directives(src);
            assert_eq!(rendered(&m).trim_end(), src.trim_end(), "prose was eaten: {src:?}");
        }
    }

    #[test]
    fn a_bare_colon_word_keeps_every_byte_a_caret_stop() {
        let src = "a :word b\n";
        let m = map_directives(src);
        // Nothing here is markup, so nothing is hidden: each byte maps to
        // itself and can be stood on, which is what makes the colon deletable.
        let stops: Vec<(char, usize)> = m
            .rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .filter(|g| g.stop)
            .map(|g| (g.ch, g.src))
            .collect();
        assert_eq!(stops, "a :word b".chars().enumerate().map(|(i, c)| (c, i)).collect::<Vec<_>>());
    }

    #[test]
    fn an_attribute_bearing_text_directive_draws_a_chip() {
        // `{…}` is deliberate in a way a bare colon is not — diaryx writes
        // `:vis{.family}` inline — so this one reads as an embed, on the same
        // `⧉ label` recipe the leaf form's placeholder row uses.
        // Both attribute conventions label it: twig's dot-prefixed classes and
        // the bare pandoc-style words diaryx also writes.
        for src in ["a :vis{.family} b\n", "a :vis{family} b\n"] {
            let m = map_directives(src);
            assert_eq!(rendered(&m).trim_end(), "a ⧉ vis family b", "{src:?}");
        }
        // A `key=value` attr is configuration, not a name, so it adds nothing.
        let m = map_directives("a :foo{title=\"x\"} b\n");
        assert_eq!(rendered(&m).trim_end(), "a ⧉ foo b");
    }

    #[test]
    fn a_directive_chip_is_one_atomic_caret_stop_at_its_own_offset() {
        let src = "a :vis{.family} b\n";
        let m = map_directives(src);
        let stops: Vec<usize> =
            m.rows.iter().flat_map(|r| &r.glyphs).filter(|g| g.stop).map(|g| g.src).collect();
        // The chip contributes exactly one stop, at the directive's start (2),
        // so the caret steps over it whole instead of walking hidden markup a
        // byte at a time. `{.family}`'s bytes (3..15) are never stood on.
        assert_eq!(stops, [0, 1, 2, 15, 16]);
    }

    #[test]
    fn a_paragraph_holding_only_a_chip_is_still_navigable() {
        // With no stop of its own the row would be unreachable — the caret
        // could never be put on the line to edit or delete the directive.
        let m = map_directives(":vis{.family}\n");
        assert!(m.row_is_navigable(0), "a chip-only paragraph has no caret home");
        assert_eq!(m.offset_of_pos(0, 0), 0, "its caret home isn't the directive's start");
    }

    #[test]
    fn a_ratio_or_a_clock_time_is_never_a_directive() {
        // twig requires a letter after the colon, so these stay prose — the
        // verbatim arm must not be reached for them at all.
        let src = "ratio 3:4 and 10:30\n";
        assert_eq!(rendered(&map_directives(src)).trim_end(), "ratio 3:4 and 10:30");
    }

    #[test]
    fn a_leaf_directive_is_a_placeholder_row_with_its_attrs_published() {
        // `::name{…}` is a standalone block with no body — an embed, a table of
        // contents. It used to emit no rows at all: invisible, no caret home,
        // vertical motion crossing a void. Now it draws the image recipe's
        // placeholder and publishes what the host app needs to paint the real
        // thing.
        let src = "before\n\n::embed{src=\"demo.html\" height=\"400\"}\n\nafter\n";
        let m = map_directives(src);

        let row = m.rows.iter().position(|r| r.leaf_directive.is_some()).expect("a placeholder row");
        assert_eq!(m.rows[row].glyphs.iter().map(|g| g.ch).collect::<String>(), "⧉ embed");
        assert!(m.rows[row].glyphs.iter().any(|g| g.stop), "the caret can land on it");
        assert!(m.rows[row].directive, "a frontend frames it like the container form");

        assert_eq!(m.directives.len(), 1);
        let info = &m.directives[0];
        assert_eq!(info.name, "embed");
        assert_eq!(info.rows_span, row..row + 1);
        assert_eq!(info.attr("src"), Some("demo.html"));
        assert_eq!(info.attr("height"), Some("400"));
        assert_eq!(info.attr("nope"), None);
        // The prose around it is untouched.
        assert!(rendered(&m).contains("before") && rendered(&m).contains("after"));
    }

    #[test]
    fn a_leaf_directive_shows_its_label_and_honours_its_prefix() {
        // A `[label]` names the placeholder (the way an image's alt does), and a
        // quoted directive keeps the quote's gutter — it is a block like any
        // other, not a special case that escapes its container.
        let m = map_directives("::embed[Audience demo]{src=\"demo.html\"}\n");
        assert_eq!(rendered(&m).trim_end(), "⧉ Audience demo");
        assert_eq!(m.directives[0].label, "Audience demo");

        let quoted = map_directives("> ::embed{src=\"x.html\"}\n");
        assert_eq!(rendered(&quoted).trim_end(), "│ ⧉ embed");
        assert_eq!(quoted.directives[0].name, "embed");
    }

    #[test]
    fn a_container_directive_is_still_a_panel_not_a_placeholder() {
        // The three forms must not bleed into each other: only the leaf form is
        // a placeholder, and only the container form tints the blocks it wraps.
        let m = map_directives(":::note{.warning}\nBody\n:::\n");
        assert!(m.directives.is_empty(), "a container publishes no placeholder");
        assert!(m.rows.iter().all(|r| r.leaf_directive.is_none()));
        assert_eq!(rendered(&m).trim_end(), "Body");
        assert!(m.rows.iter().any(|r| r.directive && r.directive_label.as_deref() == Some("warning")));
    }

    /// A production-path build with both extensions on — the only way to put a
    /// promoted HTML element and a directive in one document, which is what the
    /// `container` kind made necessary to tell apart. Returns the whole `Doc`
    /// because [`VisualMap`] is not `Clone`; read `doc.vmap`.
    fn doc_built(src: &str) -> crate::Doc {
        let mut doc = crate::Doc::from_source(src.to_string(), Format::Markdown).unwrap();
        doc.build_visual(80);
        doc
    }

    /// Every `container` node in `src`, parsed the way production does (both
    /// extensions on), paired with what [`container_is_directive`] makes of it.
    fn containers(src: &str) -> Vec<(String, bool, Option<DirectiveForm>)> {
        let mut ed = Editor::new_ext(
            src.as_bytes(),
            Format::Markdown,
            twig::MarkdownExtensions {
                directives: true,
                html_elements: true,
                ..Default::default()
            },
        )
        .unwrap();
        ed.nodes()
            .unwrap()
            .iter()
            .filter(|n| n.kind == Kind::Container)
            .map(|n| {
                (
                    n.name.clone().unwrap_or_default(),
                    container_is_directive(n),
                    n.directive_form,
                )
            })
            .collect()
    }

    #[test]
    fn a_directive_and_an_html_element_are_told_apart_by_spelling_not_by_form() {
        // twig 2.8 folded `div`/`span`/`directive`/`element` into one `container`
        // kind. `directive_form` reads as though it separates them and does not:
        // a block-level `<div>` reports `Some(DirectiveForm::Container)` exactly
        // as a `:::note` does. Trusting it would draw directive chrome — a tinted
        // panel, a `.class` audience label — on every pasted Slack/Docs div.
        for (src, name, want) in [
            (":::note{.a}\nbody\n:::\n", "note", true),
            ("::embed{src=x}\n", "embed", true),
            ("a :vis[hi]{.b} b\n", "vis", true),
            ("<div class=\"x\">\nhi\n</div>\n", "div", false),
            ("<video src=\"v.mp4\" controls></video>\n", "video", false),
            ("<audio src=\"a.mp3\" controls></audio>\n", "audio", false),
            ("<figure>\n\nhi\n\n</figure>\n", "figure", false),
            // The `:` in an attribute must not read as a directive opener: the
            // `<` of the tag comes first, and first one wins.
            ("<video src=\"http://x.test/v.mp4\" controls></video>\n", "video", false),
            ("<source media=\"(prefers-color-scheme: dark)\" srcset=\"d.svg\">\n", "source", false),
        ] {
            let found = containers(src);
            let hit = found.iter().find(|(n, ..)| n == name);
            let Some((_, is_directive, form)) = hit else {
                panic!("no `{name}` container in {src:?} — found {found:?}");
            };
            assert_eq!(*is_directive, want, "{name} in {src:?} (form was {form:?})");
        }

        // And the reason this can't just read the field: for the one collision
        // that matters, the field says the same thing for both.
        let div = containers("<div class=\"x\">\nhi\n</div>\n");
        let note = containers(":::note{.a}\nbody\n:::\n");
        assert_eq!(
            div[0].2, note[0].2,
            "if these ever differ, `directive_form` became usable and this rule can go"
        );
    }

    #[test]
    fn a_directive_nested_in_a_quote_or_list_is_still_a_directive() {
        // A container's span opens with its *block prefix*, not its own markup —
        // `> ::embed{…}` starts at the `>`. Reading only the first byte to tell a
        // directive from an element (both `container` since 2.8) therefore misses
        // every nested one, and the placeholder silently renders as nothing.
        for (src, ctx) in [
            ("> ::embed{src=\"x\"}\n", "quoted"),
            ("- ::embed{src=\"x\"}\n", "listed"),
            (">> ::embed{src=\"x\"}\n", "twice quoted"),
        ] {
            let m = map_directives(src);
            assert_eq!(m.directives.len(), 1, "{ctx} directive was lost");
            assert_eq!(m.directives[0].name, "embed", "{ctx}");
        }
    }

    #[test]
    fn a_video_is_still_media_and_not_a_directive() {
        // The other side of the same coin: `<video>` is a `container` too, and
        // must reach `block_media` rather than the directive arms.
        let doc = doc_built("<video src=\"clip.mp4\" controls></video>\n");
        assert_eq!(doc.vmap.media.len(), 1, "the video is block media");
        assert!(doc.vmap.rows.iter().all(|r| !r.directive), "the video drew directive chrome");
    }

    #[test]
    fn a_directive_needs_the_extension_flag() {
        // `map` (twig's default extensions) leaves `directives` off — the fence
        // renders as literal paragraph text, same as any other unrecognized
        // punctuation, never corrupting or panicking.
        let src = ":::vis{.public}\nhello\n:::\n";
        let m = map(src);
        assert!(m.rows.iter().all(|r| !r.directive));
        assert!(rendered(&m).contains(":::vis{.public}"));
    }

    #[test]
    fn a_footnote_reference_keeps_its_paragraph_visible() {
        // Regression: `footnote_reference` was in neither `is_inline_kind` nor
        // the inline walker, so a paragraph carrying one failed the "all children
        // inline" test, was walked as a container of blocks, and rendered as
        // empty rows with no caret stop anywhere — the whole line vanished.
        let src = "A claim[^1] and more.\n";
        let m = map(src);
        assert_eq!(rendered(&m).trim_end(), "A claim[1] and more.");
        // The `^` is spelling, not text: hidden the way a link's `](dest)` is.
        assert!(!rendered(&m).contains('^'));
    }

    #[test]
    fn a_footnote_reference_is_raised_and_the_prose_around_it_is_not() {
        // What makes `[1]` read as a reference rather than as bracketed text.
        // The brackets ride with the label: the chip is one raised mark.
        let m = map("A claim[^1] and more.\n");
        assert_eq!(baselines_of(&m, '1'), vec![Baseline::Super]);
        assert_eq!(baselines_of(&m, '['), vec![Baseline::Super]);
        assert_eq!(baselines_of(&m, ']'), vec![Baseline::Super]);
        assert_eq!(baselines_of(&m, 'A'), vec![Baseline::Normal]);
    }

    #[test]
    fn a_footnote_reference_keeps_the_link_role_it_had() {
        // The raised baseline is added to the role, not swapped for it: every
        // frontend already paints `Role::Link`, and a reference is one.
        let m = map("A claim[^1].\n");
        let label = m.rows.iter().flat_map(|r| &r.glyphs).find(|g| g.ch == '1').unwrap();
        assert_eq!(label.style.role, Role::Link);
        assert_eq!(label.style.baseline, Baseline::Super);
    }

    #[test]
    fn a_superscript_and_a_subscript_sit_off_the_baseline() {
        // Regression: both rendered flat, so the toolbar's superscript button
        // produced markup that looked exactly like the text around it.
        let m = map_djot("H~2~O and x^2^\n");
        assert_eq!(baselines_of(&m, '2'), vec![Baseline::Sub, Baseline::Super]);
        assert_eq!(baselines_of(&m, 'H'), vec![Baseline::Normal]);
        assert_eq!(baselines_of(&m, 'O'), vec![Baseline::Normal]);
    }

    #[test]
    fn a_raised_glyph_keeps_the_style_it_was_raised_out_of() {
        // Why this is a `Baseline` and not a `Role`: raising a glyph says where
        // it sits, and must not cost it what it already was.
        let m = map_djot("# Heading x^2^\n");
        let two = m.rows.iter().flat_map(|r| &r.glyphs).find(|g| g.ch == '2').unwrap();
        assert_eq!(two.style.baseline, Baseline::Super);
        assert_eq!(two.style.role, Role::Heading(1), "still heading text");
    }

    #[test]
    fn a_footnote_references_brackets_are_decoration_and_only_its_label_is_a_stop() {
        let src = "see[^note] here\n";
        let m = map(src);
        // `[^note]` spans 3..10, its label `note` 5..9. The caret walks the
        // label; the brackets are drawn but never stood on, as a table's are,
        // and the `[^`/`]` bytes are stepped over like any hidden delimiter.
        let stops: Vec<usize> =
            m.rows.iter().flat_map(|r| &r.glyphs).filter(|g| g.stop).map(|g| g.src).collect();
        for off in 5..9 {
            assert!(stops.contains(&off), "label byte {off} isn't a caret stop: {stops:?}");
        }
        for off in [3usize, 4, 9] {
            assert!(!stops.contains(&off), "delimiter byte {off} is a caret stop: {stops:?}");
        }
    }

    #[test]
    fn a_task_item_draws_its_box_where_the_bullet_would_be() {
        // Regression: the `[ ] ` is markup twig consumes — the item's paragraph
        // content starts past it — so a task item used to render as `• todo`,
        // identical to a plain bullet and with no way to see it was ticked.
        let m = map("- [ ] todo\n- [x] done\n- plain\n");
        assert_eq!(rendered(&m), "☐ todo\n☑ done\n• plain");

        // The tick rides the item's first row, for a GUI that paints its own box.
        let ticks: Vec<Option<bool>> = m.rows.iter().map(|r| r.task).collect();
        assert_eq!(ticks, [Some(false), Some(true), None]);
    }

    #[test]
    fn a_task_items_box_survives_a_wrap_and_marks_only_the_first_row() {
        let m = map_at("- [x] a much longer task that has to wrap somewhere\n", Some(20));
        assert!(m.rows.len() > 1, "the item should wrap: {:?}", rendered(&m));
        assert_eq!(m.rows[0].task, Some(true));
        assert!(m.rows[1..].iter().all(|r| r.task.is_none()), "only the first row");
        // The continuation lines hang under the box, not under column zero.
        assert!(rendered(&m).lines().nth(1).is_some_and(|l| l.starts_with("  ")));
    }

    #[test]
    fn a_bracket_in_an_items_prose_is_not_a_checkbox() {
        // `task_checked` finds the box past the list marker; a plain item whose
        // text merely contains a bracket has none, and must keep its bullet.
        let m = map("- see [1] below\n");
        assert_eq!(rendered(&m), "• see [1] below");
        assert_eq!(m.rows[0].task, None);
    }

    #[test]
    fn a_footnote_definition_renders_where_it_was_written() {
        // Regression: twig parses `[^1]: …` as a root *beside* `doc` — not a
        // child of it — so the walk from `doc` never reached one and every byte
        // of the note's body rendered as nothing at all.
        let src = "A claim[^1].\n\n[^1]: The note body.\n\nAfter.\n";
        let m = map(src);
        let text = rendered(&m);
        assert!(text.contains("The note body."), "the note body is invisible: {text:?}");
        // In source order — between the paragraph that cites it and the one
        // after — not hoisted to the end, and marked to match its reference.
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines, ["A claim[1].", "[1] The note body.", "After."]);
    }

    #[test]
    fn a_footnote_definitions_body_maps_to_its_own_source_bytes() {
        let src = "x[^a].\n\n[^a]: body\n";
        let m = map(src);
        // `body` sits at 14..18. Its glyphs must map there — a marker that ate
        // the offsets would put the caret in the wrong place on every click.
        let body: Vec<(char, usize)> = m
            .rows
            .iter()
            .flat_map(|r| &r.glyphs)
            .filter(|g| g.stop && g.src >= 14)
            .map(|g| (g.ch, g.src))
            .collect();
        assert_eq!(body, [('b', 14), ('o', 15), ('d', 16), ('y', 17)]);
    }

    #[test]
    fn an_empty_footnote_definition_still_shows_its_marker() {
        // The instant `[^1]: ` has been typed and nothing after it. `blocks`
        // renders no child, so without the explicit marker row the definition
        // wouldn't appear at all until something was typed into it.
        let src = "x[^1]\n\n[^1]:\n";
        let m = map(src);
        assert!(rendered(&m).contains("[1] "), "no marker row: {:?}", rendered(&m));
    }

    #[test]
    fn a_footnote_definition_wearing_a_long_label_indents_its_wrapped_body() {
        let src = "x[^src]\n\n[^src]: one two three four five six seven\n";
        let m = map_at(src, Some(24));
        let text = rendered(&m);
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        // Continuation lines hang under the marker, as a list item's do — the
        // indent is the marker's own width, not a fixed one.
        assert_eq!(lines[1].trim_end(), "[src] one two three four");
        assert!(lines[2].starts_with("      "), "body doesn't hang: {:?}", lines[2]);
        assert_eq!(lines[2].trim(), "five six seven");
    }

    #[test]
    fn a_code_block_leaves_exactly_one_blank_row_below_it() {
        // The closing fence line used to be miscounted as a blank separator,
        // opening a phantom second gap under the block. One block boundary is
        // one blank row, code block or not.
        let src = "para\n\n```\ncode\n```\n\nafter\n";
        let m = map(src);
        let code_end = m.code_blocks[0].rows_span.end;
        let after = m
            .rows
            .iter()
            .position(|r| r.glyphs.iter().map(|g| g.ch).collect::<String>() == "after")
            .unwrap();
        assert_eq!(after - code_end, 1, "exactly one row between code and 'after'");
    }

    #[test]
    fn a_fenced_block_publishes_its_language_on_its_code_block() {
        // The info string becomes the block's label; a bare fence and an indented
        // block carry none.
        assert_eq!(
            map("```rust\nlet x = 1;\n```\n").code_blocks[0].lang.as_deref(),
            Some("rust")
        );
        assert_eq!(map("```\nplain\n```\n").code_blocks[0].lang, None);
        assert_eq!(map("    indented\n").code_blocks[0].lang, None);
    }

    #[test]
    fn inline_code_is_not_a_code_block() {
        // A `code` span inside prose is styled by role, not boxed: it's part of a
        // normal paragraph row, so it names no `code_blocks` entry.
        let m = map("a `snippet` b\n");
        assert!(m.code_blocks.is_empty(), "inline code wrongly boxed");
        assert!(m.rows.iter().all(|r| !r.code), "inline code flagged a code row");
    }

    #[test]
    fn caret_steps_over_hidden_delimiters() {
        // "a **bold** c": bytes 8,9 are the closing ** — no glyph. Moving right
        // from 'd' (src 7) lands on the space before 'c' (src 10), not inside **.
        let m = map("a **bold** c\n");
        let (r, c) = m.pos_of_offset(7);
        assert_eq!(m.offset_of_pos(r, c + 1), 10);
    }

    // ── the structural view of a table ───────────────────────────────────────

    #[test]
    fn a_table_is_published_structurally_beside_its_picture() {
        let m = map(TABLE);
        let t = &m.tables[0];
        let cell = |r: usize, c: usize| -> String {
            t.grid[r].cells[c].glyphs.iter().map(|g| g.ch).collect()
        };
        assert_eq!(t.grid.len(), 3, "head + two body rows");
        assert_eq!(
            (cell(0, 0), cell(0, 1), cell(1, 0), cell(2, 1)),
            ("Name".into(), "Qty".into(), "Pear".into(), "12".into())
        );
        assert_eq!(
            t.grid.iter().map(|r| r.head).collect::<Vec<_>>(),
            [true, false, false]
        );
        // The alignment the delimiter row spelled, carried per cell — the only
        // place it survives, since the parser consumes that row.
        assert!(matches!(t.grid[1].cells[0].align, Alignment::Left));
        assert!(matches!(t.grid[1].cells[1].align, Alignment::Right));
    }

    #[test]
    fn a_block_media_is_published_structurally_beside_its_placeholder() {
        let m = map("intro\n\n![a cat](img/cat.png)\n\nend\n");
        assert_eq!(m.media.len(), 1, "one block image");
        let img = &m.media[0];
        assert_eq!(img.destination, "img/cat.png");
        assert_eq!(img.alt, "a cat");
        // The placeholder row named by `rows_span` carries the label a plain
        // surface paints and a capable frontend replaces.
        let row_text = |r: usize| -> String {
            m.rows[r].glyphs.iter().map(|g| g.ch).collect()
        };
        assert_eq!(img.rows_span.end - img.rows_span.start, 1, "one placeholder row");
        assert_eq!(row_text(img.rows_span.start), "🖼 a cat");
        // The row carries the mark `media_spans` derives the side-table from.
        assert!(m.rows[img.rows_span.start].media.is_some());
    }

    #[test]
    fn an_image_without_alt_labels_itself_with_its_filename() {
        let m = map("![](photos/beach.jpg)\n");
        let row = &m.rows[m.media[0].rows_span.start];
        assert_eq!(row.glyphs.iter().map(|g| g.ch).collect::<String>(), "🖼 beach.jpg");
        assert_eq!(m.media[0].alt, "");
    }

    #[test]
    fn a_block_media_gives_the_caret_a_home_before_and_after_it() {
        // `![x](y)` on its own line: the caret can rest in front of the image
        // (its start) and just past it (the row end), and nowhere inside the
        // markup — the same coarse mapping a thematic break uses.
        let src = "![x](y.png)\n";
        let m = map(src);
        let img = &m.rows[m.media[0].rows_span.start];
        let start = 0; // the image opens the document
        let end = "![x](y.png)".len();
        // Every placeholder glyph maps to the image start and is a stop there.
        assert!(img.glyphs.iter().all(|g| g.src == start && g.stop));
        assert_eq!(img.end_src, end, "the row ends past the image");
        assert_eq!(m.stops.first(), Some(&start));
        assert!(m.stops.contains(&end), "a stop sits after the image");
        // Nothing inside the markup is a stop.
        assert!(!m.stops.iter().any(|&s| s > start && s < end));
    }

    #[test]
    fn an_inline_image_amid_text_is_not_a_block_media() {
        // An image sharing its line with prose isn't block-level: it stays in the
        // inline path (rendered as its alt text), and publishes no MediaInfo.
        let m = map("see ![a cat](cat.png) here\n");
        assert!(m.media.is_empty(), "not a block image");
        assert!(rendered(&m).contains("a cat"), "alt text still renders inline");
    }

    /// The block images `Doc` publishes for `src`, driven through the real
    /// production build (`build_visual` → `build_cached`) with `html_elements`
    /// on — the path a `<picture>` actually travels. Not the raw `build` the
    /// other tests use: the editor's flat whole-arena snapshot tangles the links
    /// of inline-promoted HTML (phantom roots, dangling `parent`s), which only
    /// the per-block subtree walk `build_cached` does untangles.
    fn doc_media(src: &str) -> Vec<MediaInfo> {
        let mut doc = crate::Doc::from_source(src.to_string(), Format::Markdown).unwrap();
        doc.build_visual(80);
        doc.vmap.media.clone()
    }

    #[test]
    fn a_video_block_is_media_with_its_src_poster_and_kind() {
        // The load-bearing assumption of video support: twig has no `video` node
        // kind, so `html_elements` promotion must land a `<video>` as a generic
        // `element` whose tag name and attributes survive onto `FlatNode` — the
        // same treatment `<picture>` gets. If that ever stops holding, this is
        // the test that says so.
        let m = doc_media("<video src=\"clip.mp4\" poster=\"still.png\" controls>\n</video>\n");
        assert_eq!(m.len(), 1, "the video is one block media");
        assert_eq!(m[0].kind, MediaKind::Video);
        assert_eq!(m[0].destination, "clip.mp4");
        assert_eq!(m[0].poster, "still.png");
    }

    #[test]
    fn a_single_line_video_is_a_block_too() {
        // The spelling everyone actually writes. It used to parse as a paragraph
        // of raw inline HTML — CommonMark opens a block on a complete tag only
        // when the line ends there, and its fixed tag list predates `<video>` —
        // so the tags never reached core as an element at all. twig 2.5.1 widened
        // that list under `html_elements`; this is the test that would catch the
        // pin sliding back.
        let m = doc_media("<video src=\"clip.mp4\" controls></video>\n");
        assert_eq!(m.len(), 1, "single-line <video> is a block");
        assert_eq!(m[0].kind, MediaKind::Video);
        assert_eq!(m[0].destination, "clip.mp4");
    }

    #[test]
    fn a_single_line_picture_is_a_block_with_its_alternatives() {
        // `<picture>` had the identical gap and it went unnoticed because the
        // conventional spelling breaks the lines. Same twig fix covers it.
        let src = "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"d.svg\">\
                   <img src=\"l.svg\" alt=\"banner\"></picture>\n";
        let m = doc_media(src);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].kind, MediaKind::Image);
        assert_eq!(m[0].destination, "l.svg");
        assert_eq!(m[0].resolve(ColorScheme::Dark), "d.svg");
    }

    #[test]
    fn an_audio_block_is_media_with_no_poster() {
        let m = doc_media("<audio src=\"take.mp3\" controls>\n</audio>\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].kind, MediaKind::Audio);
        assert_eq!(m[0].destination, "take.mp3");
        assert!(m[0].poster.is_empty(), "audio has no poster frame");
    }

    #[test]
    fn a_videos_source_children_are_its_candidates_typed_by_mime() {
        // A `<video>` with no `src` of its own — the common shape, since it's how
        // you offer more than one codec. The candidates come from `<source src>`
        // (not `srcset`, which is `<picture>`'s spelling) and carry their MIME.
        let src = "<video controls>\n\
                   <source src=\"a.webm\" type=\"video/webm\">\n\
                   <source src=\"a.mp4\" type=\"video/mp4\">\n\
                   fallback\n\
                   </video>\n";
        let m = doc_media(src);
        assert_eq!(m.len(), 1);
        assert!(m[0].destination.is_empty(), "no src attribute on the element");
        assert_eq!(m[0].sources.len(), 2);
        assert_eq!(m[0].sources[0].srcset, "a.webm");
        assert_eq!(m[0].sources[0].mime, "video/webm");
        assert_eq!(m[0].sources[1].srcset, "a.mp4");
        // With an empty destination, `resolve` falls through to the first
        // candidate rather than handing the frontend nothing to load.
        assert_eq!(m[0].resolve(ColorScheme::Light), "a.webm");
    }

    #[test]
    fn a_video_placeholder_row_carries_its_own_sigil_and_mark() {
        // The placeholder contract images already hold, now for a video: the row
        // renders as a labelled stand-in a plain surface can paint as-is, and
        // carries the mark a capable frontend replaces it from.
        let src = "<video src=\"clip.mp4\" controls>\n</video>\n";
        let mut doc = crate::Doc::from_source(src.to_string(), Format::Markdown).unwrap();
        doc.build_visual(80);
        let row = &doc.vmap.rows[doc.vmap.media[0].rows_span.start];
        let text: String = row.glyphs.iter().map(|g| g.ch).collect();
        assert!(text.starts_with('🎬'), "video sigil, not the image one: {text:?}");
        assert!(row.media.is_some(), "the mark rides the placeholder row");
    }

    #[test]
    fn a_picture_block_carries_its_source_alternatives() {
        // A `<picture>` with a dark-mode `<source>`: one block image, whose
        // fallback destination is the `<img>` and whose `sources` carry the
        // `<source>`'s media + srcset for a theme-aware frontend to pick.
        let src = "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.svg\"><img src=\"light.svg\" alt=\"banner\"></picture>\n";
        let images = doc_media(src);
        assert_eq!(images.len(), 1, "the picture is one block image");
        let img = &images[0];
        assert_eq!(img.destination, "light.svg", "fallback is the <img>");
        assert_eq!(img.alt, "banner");
        assert_eq!(
            img.sources,
            vec![MediaSource {
                media: "(prefers-color-scheme: dark)".into(),
                srcset: "dark.svg".into(),
                mime: String::new(),
            }],
        );
    }

    #[test]
    fn a_picture_inside_a_heading_is_still_a_block_media_with_sources() {
        // fig.md's shape: the banner is an `<h1>` wrapping the `<picture>`.
        let src = "<h1><picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"d.svg\"><img src=\"l.svg\" alt=\"fig\"></picture></h1>\n";
        let images = doc_media(src);
        assert_eq!(images.len(), 1, "heading-wrapped picture is a block image");
        assert_eq!(images[0].destination, "l.svg");
        assert_eq!(images[0].sources.len(), 1);
        assert_eq!(images[0].sources[0].srcset, "d.svg");
    }

    #[test]
    fn a_plain_image_has_no_media_sources() {
        // A bare Markdown image carries an empty `sources` — nothing to pick from.
        let images = doc_media("![alt](p.png)\n");
        assert_eq!(images.len(), 1);
        assert!(images[0].sources.is_empty(), "no <picture>, no alternatives");
    }

    #[test]
    fn resolve_picks_the_source_matching_the_scheme() {
        let src = "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"dark.svg\"><img src=\"light.svg\" alt=\"b\"></picture>\n";
        let images = doc_media(src);
        let img = &images[0];
        // Dark theme takes the dark source; light falls through to the <img>.
        assert_eq!(img.resolve(ColorScheme::Dark), "dark.svg");
        assert_eq!(img.resolve(ColorScheme::Light), "light.svg");
    }

    #[test]
    fn resolve_falls_back_for_a_plain_image_and_unknown_media() {
        // A plain image ignores the scheme.
        let plain = doc_media("![a](p.png)\n");
        assert_eq!(plain[0].resolve(ColorScheme::Dark), "p.png");

        // A <source> with an unrecognized media query is skipped; a light source
        // is taken under a light theme.
        let m = doc_media("<picture><source media=\"print\" srcset=\"p.svg\"><source media=\"(prefers-color-scheme: light)\" srcset=\"l.svg\"><img src=\"f.svg\" alt=\"x\"></picture>\n");
        assert_eq!(m[0].resolve(ColorScheme::Light), "l.svg");
        assert_eq!(m[0].resolve(ColorScheme::Dark), "f.svg", "no dark source → <img>");
    }

    #[test]
    fn resolve_reads_the_first_srcset_url_ignoring_descriptors() {
        // A comma/descriptor srcset resolves to its first URL.
        assert_eq!(first_srcset_url("a.png 1x, b.png 2x"), Some("a.png"));
        assert_eq!(first_srcset_url("  solo.svg  "), Some("solo.svg"));
        assert_eq!(first_srcset_url(""), None);
        // An empty (unconditional) media always matches.
        assert!(media_matches("", ColorScheme::Light));
        assert!(media_matches("(prefers-color-scheme:dark)", ColorScheme::Dark));
        assert!(!media_matches("(prefers-color-scheme: dark)", ColorScheme::Light));
    }

    #[test]
    fn a_block_media_carries_its_list_prefix() {
        // An image that is a list item's body opens past the bullet, like every
        // other block does.
        let m = map("- ![alt](p.png)\n");
        let row = &m.rows[m.media[0].rows_span.start];
        let text: String = row.glyphs.iter().map(|g| g.ch).collect();
        assert!(text.starts_with("• "), "the list marker prefixes the image row: {text:?}");
        assert!(text.contains("🖼 alt"));
    }

    #[test]
    fn the_structural_table_spans_exactly_its_drawn_rows() {
        // A frontend drawing its own grid skips `rows_span` and renders from
        // `grid`. If the span were short the leftover border rows would be
        // painted as text under the real table; if long it would eat a
        // neighbouring paragraph. Both are silent, so pin it to the picture.
        let m = map(&format!("before\n\n{TABLE}\nafter\n"));
        let t = &m.tables[0];
        let row_text = |r: usize| -> String { m.rows[r].glyphs.iter().map(|g| g.ch).collect() };
        assert!(row_text(t.rows_span.start).starts_with('┌'), "opens on the top border");
        assert!(
            row_text(t.rows_span.end - 1).starts_with('└'),
            "closes on the bottom border"
        );
        assert!(
            !row_text(t.rows_span.start - 1).contains('┌'),
            "the row before the span is not the table's"
        );
        assert_eq!(row_text(t.rows_span.end), "", "the span ends before the gap row");
    }

    #[test]
    fn a_nested_tables_structure_carries_the_block_prefix() {
        // The picture puts the quote's gutter on every row of the grid. A
        // frontend drawing its own table has to draw that too and start past it,
        // so the prefix has to travel with the structure — without it a quoted
        // table renders flush at the margin and leaves the quote it's in.
        let m = map("> | a | b |\n> |---|---|\n> | c | d |\n");
        let t = &m.tables[0];
        let prefix: String = t.prefix.iter().map(|g| g.ch).collect();
        assert_eq!(prefix, "│ ", "the quote's gutter should ride the structure");
        // And it matches what the picture actually drew.
        let drawn: String = m.rows[t.rows_span.start].glyphs.iter().map(|g| g.ch).collect();
        assert!(drawn.starts_with(&prefix), "picture and structure disagree: {drawn:?}");
    }

    #[test]
    fn a_top_level_table_carries_no_prefix() {
        assert!(map(TABLE).tables[0].prefix.is_empty());
    }

    #[test]
    fn structural_cells_are_unwrapped_even_when_the_picture_wraps_them() {
        // The picture wraps a cell to its column; a frontend laying the grid out
        // in pixels needs the text as the document spells it, before that
        // decision. Narrow enough that the drawn cell must break.
        let src = "| Name |\n|------|\n| alpha beta gamma |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build_t(&ed.nodes().unwrap(), src, Some(12));
        let drawn = rendered(&m);
        let cell: String = m.tables[0].grid[1].cells[0]
            .glyphs
            .iter()
            .map(|g| g.ch)
            .collect();
        assert_eq!(cell, "alpha beta gamma", "structure must not carry the wrap");
        assert!(
            drawn.lines().count() > 5,
            "the picture should have wrapped, else this proves nothing:\n{drawn}"
        );
    }

    // ── display columns ──────────────────────────────────────────────────────

    #[test]
    fn a_table_column_is_as_wide_as_its_cells_are_drawn() {
        // A column sized by counting characters is drawn narrower than the text
        // it has to hold — `你好` is two characters in four cells — and the cell
        // spills over the border it is supposed to sit inside, taking the whole
        // grid out of square with it. Squareness is the property: every row of a
        // grid is drawn to the same column, whatever its cells are spelled with.
        for src in [
            "| A | B |\n|---|---|\n| 你好 | y |\n",
            "| A | B |\n|---|---|\n| a👨‍👩‍👧b | y |\n",
            "| A | 漢字 |\n|---|---|\n| x | y |\n",
        ] {
            let m = map(src);
            let widths: Vec<usize> = m.rows.iter().map(|r| r.width()).collect();
            assert!(
                widths.windows(2).all(|w| w[0] == w[1]),
                "ragged grid {widths:?} for {src:?}:\n{}",
                rendered(&m)
            );
        }
    }

    #[test]
    fn a_cell_wrapped_narrow_never_breaks_inside_a_character() {
        // A column too narrow for its cell hard-breaks the text, and every line
        // of it is given an end stop just past its last glyph. Broken into runs
        // of four glyphs, the first line of this cell ends between `👨‍👩` and the
        // joiner holding `👧` on — so its end stop lands inside a character,
        // where a click or Down can reach it and the next Backspace takes the
        // cluster apart from the middle.
        let src = "| A |\n|---|\n| 👨‍👩‍👧👨‍👩‍👧 |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build_t(&ed.nodes().unwrap(), src, Some(8));
        let boundaries: Vec<usize> = src
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(src.len()))
            .collect();
        for off in (0..=src.len()).filter(|&o| m.is_stop(o)) {
            assert!(
                boundaries.contains(&off),
                "stop at {off} is inside a character:\n{}",
                rendered(&m)
            );
        }
    }

    #[test]
    fn a_wrapped_cell_keeps_every_line_inside_its_column() {
        // The width is a promise in a table, where a glyph past the column lands
        // on the border or in the next cell — and it is a promise about cells,
        // which is not what a count of glyphs measures.
        let src = "| A |\n|---|\n| 你好世界漢字 |\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let m = build_t(&ed.nodes().unwrap(), src, Some(14));
        for r in &m.rows {
            assert_eq!(r.width(), 14, "{:?} is not drawn to the grid", rendered(&m));
        }
    }

    #[test]
    fn a_hard_break_falls_between_clusters_and_measures_in_cells() {
        let glyphs = |s: &str| {
            let mut out = Vec::new();
            push_text(&mut out, s, 0, Style::default());
            out
        };
        let piece = |p: &[Glyph]| p.iter().map(|g| g.ch).collect::<String>();

        // Six cells of CJK broken at four: two characters, then one — never
        // between the two cells of `好`.
        let w = glyphs("你好世");
        let pieces: Vec<String> = hard_break(&w, 4).iter().map(|p| piece(p)).collect();
        assert_eq!(pieces, ["你好", "世"]);

        // A character wider than the column has nowhere legal to break, so it
        // keeps its cells rather than being cut in half.
        let w = glyphs("你好");
        let pieces: Vec<String> = hard_break(&w, 1).iter().map(|p| piece(p)).collect();
        assert_eq!(pieces, ["你", "好"]);

        // An empty word yields no pieces at all — a double space stays a space.
        assert!(hard_break(&[], 4).is_empty());
    }

    #[test]
    fn an_empty_list_item_still_gets_a_bulleted_row_with_a_caret_home() {
        // Pressing Enter at the end of a list item opens a new, empty item —
        // a childless `list_item`. Without a row of its own the new bullet
        // wouldn't appear until something was typed into it (the caret would be
        // stranded on an offset no row draws). It now renders as one prefixed
        // row whose end is a caret stop, so the bullet shows and the caret lands
        // just past the marker.
        let m = map("- item\n- \n");
        assert_eq!(m.num_rows(), 2, "the empty second item needs its own row");
        assert_eq!(
            m.rows[1].glyphs.iter().map(|g| g.ch).collect::<String>(),
            "• ",
            "the empty item draws just its bullet",
        );
        // Its end is the caret home (past the `- ` marker), and it's a real stop.
        assert!(m.is_stop(m.rows[1].end_src), "the empty item's caret home is not a stop");
        assert_eq!(m.pos_of_offset(m.rows[1].end_src), (1, 2), "caret sits after '• '");
    }

    #[test]
    fn an_empty_ordered_item_gets_its_number_and_a_caret_home() {
        let m = map("1. item\n2. \n");
        assert_eq!(m.num_rows(), 2);
        assert_eq!(m.rows[1].glyphs.iter().map(|g| g.ch).collect::<String>(), "2. ");
        assert!(m.is_stop(m.rows[1].end_src));
        assert_eq!(m.pos_of_offset(m.rows[1].end_src), (1, 3), "caret sits after '2. '");
    }

    #[test]
    fn an_empty_headings_caret_home_is_past_its_hidden_marker() {
        // The toolbar's H1 on a blank line writes `# ` and nothing else. The row
        // it renders is empty (the marker is hidden), so its end *is* its only
        // caret stop — and it has to be the offset past the `# `, where typing
        // continues the heading. Anchored at the block's start instead, the caret
        // drew in front of the hashes and the first character typed there landed
        // before them (`x# `), which isn't a heading at all.
        let m = map("# \n");
        assert_eq!(m.num_rows(), 1);
        assert!(m.rows[0].glyphs.is_empty(), "the `# ` marker is hidden");
        assert_eq!(m.rows[0].end_src, 2, "the caret home is past the marker");
        assert!(m.is_stop(2), "the empty heading's caret home is not a stop");
    }

    #[test]
    fn a_headings_rows_carry_its_level_even_with_nothing_typed_in_it() {
        // The row-level fact a proportional frontend sizes a whole line by. An
        // empty heading has no glyph to read a `Role::Heading` off, so a renderer
        // scanning glyphs drew `# ` (and its caret) at body height until the
        // first character landed.
        let m = map("# \n");
        assert_eq!(m.rows[0].heading, Some(1), "the empty heading knows its level");

        // Every row of one that wraps, not just the first — and nothing else.
        let m = map_at("## a heading long enough to wrap over two rows\n\nbody\n", Some(20));
        let heads: Vec<Option<u8>> = m.rows.iter().map(|r| r.heading).collect();
        assert!(heads.iter().filter(|h| **h == Some(2)).count() >= 2, "got {heads:?}");
        assert_eq!(
            m.rows.last().and_then(|r| r.heading),
            None,
            "the paragraph under it is not a heading",
        );
    }

    #[test]
    fn an_empty_heading_leaves_the_rows_under_it_at_their_own_offsets() {
        // The row's end is also what the *next* row's separator is measured from,
        // so an empty heading that under-reported it shifted every offset below —
        // and the blank line under the heading then claimed the same offset as the
        // heading's own end. `pos_of_offset` resolves such a tie downstream (a
        // soft wrap belongs to the row below), so the caret at the end of the
        // heading was drawn two rows lower, on the blank line.
        // `text\n\n# \n\n`: the heading's content opens at 8, and the two rows
        // under it end at 9 and 10 — the blank line and the document's end.
        let m = map("text\n\n# \n\n");
        let end = m.rows.last().expect("a trailing blank row").end_src;
        assert_eq!(end, 10, "the trailing rows must end at their real offsets");
        // The heading's caret home is its own row's, not one shared with a row
        // below — the tie that drew the caret two rows down.
        assert_eq!(m.pos_of_offset(8), (2, 0), "the empty heading's own row");
        assert!(m.rows[3..].iter().all(|r| r.end_src > 8), "rows below own later offsets");
    }

    // ── block boundaries ─────────────────────────────────────────────────────

    /// Every drawn boundary in `src`, in order, as `(above, below)`.
    fn boundaries(m: &VisualMap) -> Vec<(BlockClass, BlockClass)> {
        m.rows
            .iter()
            .filter_map(|r| r.boundary)
            .map(|b| (b.above, b.below))
            .collect()
    }

    #[test]
    fn a_boundary_says_which_blocks_it_divides() {
        use BlockClass::*;
        let m = map("one\n\ntwo\n\n# Head\n\ntail\n\n> quoted\n\n```\ncode\n```\n");
        assert_eq!(
            boundaries(&m),
            vec![
                (Paragraph, Paragraph),
                (Paragraph, Heading),
                (Heading, Paragraph),
                (Paragraph, Quote),
                (Quote, Code),
                // The blank the document trails off with is a boundary too — it
                // closes the last block above the empty paragraph the caret rests
                // on. See `emit_trailing_blank_lines`.
                (Code, Paragraph),
            ],
            "each gap names the pair it falls between, in document order"
        );
    }

    #[test]
    fn the_trailing_gap_closes_the_last_block() {
        // Two Enters at the end of a document: a drawn gap, then the navigable
        // empty paragraph. Only the gap is labelled, so a frontend that shrinks
        // boundaries shrinks the spacer and leaves the row being typed on alone.
        let m = map("# Head\n\n\n");
        assert_eq!(boundaries(&m), vec![(BlockClass::Heading, BlockClass::Paragraph)]);
    }

    #[test]
    fn only_the_drawn_gap_rows_carry_a_boundary() {
        let m = map("one\n\ntwo\n");
        for row in &m.rows {
            assert_eq!(
                row.boundary.is_some(),
                row.decoration,
                "a boundary is exactly a drawn gap row: {:?}",
                row.glyphs.iter().map(|g| g.ch).collect::<String>()
            );
        }
    }

    #[test]
    fn preserve_flow_labels_no_boundary() {
        // Every blank line is a caret home there — somewhere text can go, not a
        // gap between blocks — so nothing is drawn-only and nothing is labelled.
        // A frontend keying its spacing off `boundary` can't shrink a row the
        // author is about to type on.
        let m = map_preserve("one\n\ntwo\n\n# Head\n", Some(80));
        assert!(boundaries(&m).is_empty());
    }

    #[test]
    fn a_list_draws_no_boundary_between_its_items() {
        // Tight or loose, core puts no gap row between two items of one list —
        // so an item↔item boundary is a shape no frontend will ever be handed,
        // and spacing one is spacing something that isn't there.
        for src in ["- one\n- two\n", "- one\n\n- two\n"] {
            let m = map(src);
            assert!(boundaries(&m).is_empty(), "no gap row inside the list of {src:?}");
        }
        // Leaving the list is an ordinary boundary, and the list is named as
        // what sits above it.
        let m = map("- one\n- two\n\npara\n");
        assert_eq!(boundaries(&m), vec![(BlockClass::List, BlockClass::Paragraph)]);
    }

    #[test]
    fn a_nested_boundary_names_the_blocks_inside_the_container() {
        // Two paragraphs inside a blockquote are divided by a Paragraph↔Paragraph
        // boundary — the quote is the container they're both in, not what the gap
        // separates.
        let m = map("> one\n>\n> two\n");
        assert_eq!(boundaries(&m), vec![(BlockClass::Paragraph, BlockClass::Paragraph)]);
    }

    #[test]
    fn the_incremental_walk_labels_boundaries_like_the_full_one() {
        // `assert_maps_eq` compares boundaries too, so this pins the two doors
        // into `BlockClass::from_node_kind` — a `FlatNode`'s kind on the full
        // build, a query match's on the cached one — against a document with one
        // of every boundary in it.
        let src = "one\n\n# Head\n\ntwo\n\n- a\n- b\n\n> q\n\n```\nc\n```\n\npara\n";
        let mut ed = Editor::new_str(src, Format::Markdown).unwrap();
        let mut cache = BlockCache::default();
        let (full, cached) = render_both(&mut ed, src, Some(80), &mut cache);
        assert_maps_eq(&full, &cached, "boundary labelling");
        assert!(!boundaries(&full).is_empty(), "the fixture has boundaries to compare");
    }

    #[test]
    fn every_caret_stop_opens_a_cluster_of_its_row() {
        // The two ways of finding a cluster have to agree. `push_text` marks the
        // stops by segmenting one run of text; the column mapping segments the
        // whole row, decoration and all. A stop that came out as the *middle* of
        // some row-level cluster would be a caret with no column of its own —
        // drawn at the column of whatever swallowed it.
        let src = "# 標題\n\na **bold** e\u{0301}mo👨‍👩‍👧ji `x` 你好\n\n\
                   - 項目 one\n- e\u{0301}dge\n\n> 引用 text\n\n\
                   | A | 值 |\n|---|---|\n| 你好 | 👩‍🚀 |\n";
        let m = map(src);
        for (r, row) in m.rows.iter().enumerate() {
            let openers: Vec<usize> = clusters(&row.glyphs).iter().map(|c| c.glyph).collect();
            for (i, g) in row.glyphs.iter().enumerate() {
                assert!(
                    !g.stop || openers.contains(&i),
                    "row {r}: the stop at glyph {i} ({:?}) is inside a cluster, \
                     so it is drawn at another glyph's column",
                    g.ch
                );
            }
        }
    }
}
