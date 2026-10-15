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

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::OnceLock;

use twig::{Alignment, ContainerOrigin, DirectiveForm, Editor, FlatNode, Kind, QueryMatch};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::style::{
    Align, Baseline, FaceId, FaceRef, FaceTable, FontSize, LineHeight, MarkColor, Role, Style,
    TextColor, Token,
};

mod builder;
mod cache;
mod glyphs;
mod info;
mod map_build;
mod nodes;
mod queries;
mod spans;
mod visual_map;
mod width;

use builder::*;
pub use cache::*;
pub use glyphs::*;
pub use info::*;
pub use map_build::*;
pub(crate) use nodes::*;
pub(crate) use queries::*;
pub use spans::*;
pub use visual_map::*;
pub use width::*;

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

impl Glyph {
    /// The character to draw: [`ch`](Self::ch), except a list item's indent
    /// on its later rows ([`Role::ListIndent`]), which is spelled with the
    /// marker's characters for its width and drawn blank.
    pub fn drawn(&self) -> char {
        if self.style.role == Role::ListIndent {
            ' '
        } else {
            self.ch
        }
    }
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
    /// [`media`](VisualMap::media) side-table is derived from it once the rows
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
    /// them [`directive`](Self::directive) instead. Like [`media`](Self::media)
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
    /// How this row's block is aligned across the measure — the author's
    /// `class="center"`, on every row the block emits and `None` for the
    /// theme's default, which is left.
    ///
    /// A *row* fact and not a glyph one for [`heading`](Self::heading)'s reason,
    /// and more sharply: alignment is a property of the *line*, not of the
    /// letters on it, so an empty paragraph the author has just centred has to
    /// carry it with no glyph to hang it on. It rides the row like a plain
    /// `Copy` flag, so [`BlockCache`] reuse and [`build_spliced`] carry it
    /// untouched.
    ///
    /// Read from the paragraph's or heading's own attributes and from those of
    /// every `div` around it, the nearest winning — so `<div class="center">`
    /// around three paragraphs centres all three, which is what the author of
    /// that HTML meant.
    pub align: Option<Align>,
    /// How far apart this row's block sets its lines, as a multiple of the
    /// theme's own line height — the author's `data-line-height`, on every row
    /// the block emits and `None` for the theme's spacing.
    ///
    /// A frontend that lays rows out in pixels scales the row's height by
    /// [`LineHeight::as_f32`]; one that draws a row per terminal line ignores
    /// it, the way it ignores a heading's size. Read at the same two levels
    /// [`align`](Self::align) is, and one of the menu's three names or the
    /// exact ratio the author asked for.
    pub line_height: Option<LineHeight>,
    /// How many ems past its prefix a frontend that wraps this row itself
    /// hangs the row's later lines — a verse line's turnover, set deeper than
    /// the line it continues so a reader can tell a wrapped line from a new
    /// one. `None` on every row that is not a verse's, whose later lines hang
    /// at the prefix; `Some` on every row a verse draws, its blank lines
    /// included, which is how a frontend finds a verse's rows to mark them.
    ///
    /// The em count includes the line's own indent, which is drawn as real em
    /// spaces at the row's start rather than as prefix, so a frontend's hang
    /// is its measured prefix plus this many ems of the body face. A frontend
    /// that lets core wrap ([`Builder::emit_line`] with a column budget) needs
    /// none of it: the continuation prefix core writes already carries it.
    pub hang: Option<u8>,
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
    /// The offsets on this row where an inline mark's *content* ends under a
    /// hidden closing delimiter — the end of the `d` in `**bold**`, one byte
    /// before the `**` that draws nothing. Each is a caret stop with no glyph
    /// of its own: the caret standing there is drawn where the next glyph is,
    /// but typing there extends the mark, where typing past the delimiter
    /// leaves it. See [`VisualMap::mark_ends`] for the rule.
    ///
    /// Source offsets, so [`shift_row`] moves them with the glyphs; empty on
    /// decoration rows and on every row no mark closes on.
    pub mark_ends: Vec<usize>,
    /// The formulas this row stands in for, in glyph order: one [`MathMark`]
    /// per inline atom on the row, or the single block mark on the
    /// placeholder row a display formula renders to. Empty on every other
    /// row, and on the revealed line, where a formula is its TeX and no
    /// picture stands for it.
    ///
    /// Plain strings and a glyph *index* rather than a source offset, for
    /// [`media`](Self::media)'s reason: the mark rides [`BlockCache`] reuse and
    /// [`build_spliced`] untouched, and the glyph it names carries the offset.
    /// The map's [`math`](VisualMap::math) side-table is derived from these
    /// once the rows are final.
    pub math: Vec<MathMark>,
    /// Set on the placeholder row of a block the frontend edits itself — see
    /// [`RegionMark`] — and `None` on every other row. The map's
    /// [`regions`](VisualMap::regions) side-table is derived from it, and from
    /// the owned [`math`](Self::math) marks, once the rows are final.
    pub region: Option<RegionMark>,
    /// The inline regions the frontend edits itself whose atoms stand on
    /// this row — a text directive (`:date[…]`) of a name it named — each
    /// with the index of its atom glyph. Empty on every other row.
    pub inline_regions: Vec<InlineRegionMark>,
}

/// An inline region the frontend edits itself, standing as one atom glyph in
/// its line — a text directive (`:name[label]{attrs}`) of a name it named in
/// [`Surface::owned_regions`], on a surface that paints pictures in a line.
/// Relative, like [`MathMark`], so a reused block's mark needs no shifting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRegionMark {
    /// The region's kind: `directive:date`.
    pub kind: String,
    /// The index into [`VRow::glyphs`] of its atom.
    pub glyph: usize,
    /// The node's length in bytes, the whole directive — which is also its
    /// content, as a leaf directive's is.
    pub len: usize,
    /// Its name, attributes and label.
    pub directive: DirectiveMark,
}

/// One formula a row stands in for — what a picture-capable frontend typesets
/// and draws in place of the glyph or the rows that hold its spot. See
/// [`VRow::math`] and, for the frontend's view of the same thing,
/// [`MathInfo`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathMark {
    /// The TeX between the delimiters, verbatim — a display block's keeps its
    /// newlines. What `leaf-math` typesets.
    pub tex: String,
    /// Display style (`$$…$$`) rather than text style (`$…$`). True for every
    /// block mark, and for a `$$…$$` written inside a line of prose, which is
    /// display style set inline.
    pub display: bool,
    /// The index into [`VRow::glyphs`] of the atom this mark stands behind —
    /// the one [`Role::Math`] glyph an inline formula renders to. `None` for
    /// a block mark, whose placeholder is the whole row.
    pub glyph: Option<usize>,
    /// How many rows a block formula reserves — the label row plus the blank
    /// fillers under it, the [`MediaMark::rows`] recipe, and from the same
    /// door: a terminal frontend that has typeset and measured the picture
    /// reports its height through [`crate::Doc::set_math_rows`]. `1` for an
    /// inline atom, which reserves nothing.
    pub rows: usize,
    /// The node's length in bytes, delimiters included — relative, like
    /// everything here, so a reused block's mark needs no shifting.
    pub len: usize,
    /// Where `tex` starts, as an offset from the node's start: past `$`, `$$`
    /// or `` $` ``, and whatever newline a display block opens with.
    pub tex_at: usize,
    /// The frontend edits formulas itself ([`Surface::owned_regions`] holds
    /// `"math"`), so this one is also published as a [`RegionInfo`].
    pub owned: bool,
}

/// A block the frontend edits itself, standing as one placeholder row — the
/// fence of a language, or the leaf directive of a name, it named in
/// [`Surface::owned_regions`]. Relative, like [`MathMark`], so a reused block's
/// mark needs no shifting. Math does not use this: its regions come from its
/// [`MathMark`]s, which every formula has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionMark {
    /// The region's kind, as the frontend named it: `fence:mermaid`,
    /// `directive:embed`.
    pub kind: String,
    /// The node's length in bytes, fences included.
    pub len: usize,
    /// Where the content starts, as an offset from the node's start.
    pub content_at: usize,
    /// The content's length in bytes.
    pub content_len: usize,
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
    ///
    /// Never reached through [`from_node_kind`](BlockClass::from_node_kind): a
    /// block picture is not a node of its own — [`Builder::media_only`] promotes
    /// the paragraph (or `<picture>`/`<video>` container) wrapping it — so the
    /// walk only ever sees the wrapper's kind. [`label_media_boundaries`] reads
    /// it back off the finished rows instead, after the fact.
    Media,
    /// A display formula on lines of its own — a paragraph holding nothing but
    /// a `$$…$$`. Never reached through [`from_node_kind`](BlockClass::from_node_kind)
    /// for [`Media`](BlockClass::Media)'s reason: the walk sees the wrapping
    /// paragraph, and [`label_media_boundaries`] relabels the gaps around the
    /// placeholder once the rows are final.
    Math,
    /// A `:::name{.class}` directive container.
    Directive,
    /// A verse — a block whose line breaks are the content. Its own class so
    /// a frontend can set it apart from the prose around it the way a book
    /// sets off an extract: a little more room above and below.
    Verse,
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
            // A verse is spaced from its neighbours as a paragraph is: it is
            // prose set in lines, and a frontend needs no class of its own to
            // put it in its place.
            Kind::Para => BlockClass::Paragraph,
            Kind::LineBlock => BlockClass::Verse,
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
            // the rest: `BlockClass` is descriptive and core never branches on
            // it. The one case where it was actively wrong — a promoted
            // `<video>`, which would have been handed to a frontend as something
            // to draw a fenced-div panel around — is corrected by
            // [`label_media_boundaries`] once the rows are final, along the same
            // door as a block image. Anything else that must be exact reads
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
    /// vertical room. `1` is the bare placeholder, and what every directive
    /// gets until a frontend that reserves rows — a terminal, which draws the
    /// host's lines over them — reports a height back through
    /// [`crate::Doc::set_directive_rows`], keyed by [`key`](Self::key). A
    /// pixel-laid-out GUI sets its own height regardless.
    pub rows: usize,
}

impl DirectiveMark {
    /// What [`crate::Doc::set_directive_rows`] knows this directive by.
    pub fn key(&self) -> DirectiveKey {
        DirectiveKey::new(&self.name, &self.label, &self.attrs)
    }
}

/// What a directive's reported height is keyed by: everything a host draws it
/// from — its name, its label and its attributes — and nothing about where it
/// stands. The peer of a picture's destination, which keys
/// [`crate::Doc::set_media_rows`]: two directives spelled alike draw alike and
/// so stand as tall, and the height survives an edit above them, which a key
/// by position would not.
///
/// An attribute's value is flattened, a bare attribute to `""`, because that is
/// the shape both bindings hand a host (`DirectiveAttr`, a string value) and so
/// the shape a host's report comes back in; the difference between `{wide}` and
/// `{wide=""}` is not one a drawing is expected to make.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DirectiveKey {
    pub name: String,
    pub label: String,
    pub attrs: Vec<(String, String)>,
}

impl DirectiveKey {
    /// The key for a directive named `name` with `label` (`""` for none) and
    /// `attrs` in source order.
    pub fn new(name: &str, label: &str, attrs: &[(String, Option<String>)]) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            attrs: attrs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone().unwrap_or_default()))
                .collect(),
        }
    }
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

/// Which of the two caret homes a block media or a block leaf directive has —
/// see [`VisualMap::block_media_stop`] and [`VisualMap::block_directive_stop`].
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
    /// core does no I/O and can't measure the image itself. See [`VRow::media`].
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

/// Assert two maps are identical down to every glyph, stop, and table span — the
/// contract `build_cached` and `build_spliced` must hold against `build`. Lives
/// at module scope (not in `mod tests`) so the Doc-driven differential test in
/// `doc/tests` can reach it and the private `stops` field it compares.
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
        assert_eq!(ra.align, rb.align, "row {i} align ({ctx})");
        assert_eq!(
            ra.line_height, rb.line_height,
            "row {i} line_height ({ctx})"
        );
        assert_eq!(
            ra.glyphs.len(),
            rb.glyphs.len(),
            "row {i} glyph count ({ctx})"
        );
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
    assert_eq!(a.mark_ends, b.mark_ends, "mark_ends ({ctx})");
    assert_eq!(a.tables.len(), b.tables.len(), "table count ({ctx})");
    for (i, (ta, tb)) in a.tables.iter().zip(&b.tables).enumerate() {
        assert_eq!(ta.rows_span, tb.rows_span, "table {i} rows_span ({ctx})");
        assert_eq!(ta.end_src, tb.end_src, "table {i} end_src ({ctx})");
    }
    assert_eq!(a.code_blocks, b.code_blocks, "code_blocks ({ctx})");
    assert_eq!(a.media, b.media, "images ({ctx})");
}

#[cfg(test)]
mod tests;
