//! What a map publishes about its tables, code blocks, media, directives,
//! math and regions, for a frontend to draw.

use super::*;

// ── helpers ──────────────────────────────────────────────────────────────────

/// The caret home inside an *empty* table cell (`col`, 0-based) whose node
/// `span` is `src` starting at byte `start`. twig gives an empty cell no
/// `content_span`, so its interior is read from the pipes: the home is one
/// space past the pipe that opens the cell — mimicking the `| ` padding a
/// filled cell has — and never at or past the pipe that closes it. So
/// `|  |  |` gives the two cells distinct, editable homes instead of both
/// collapsing onto the row's start.
///
/// Two shapes of span are read. A twig from 3.3.3 gave every cell of a row
/// the *row's* span, so the cell's own pipes are the `col`-th and
/// `col+1`-th unescaped `|` in it; a later twig spans a cell from the pipe
/// that opens it to the one that closes it, exclusive, so the span holds at
/// most that one pipe, at its start, and the closing one is the byte past
/// its end. The two are told apart by the pipes the span holds — a row's
/// span has several, or one that is not at its start.
pub(super) fn empty_cell_offset(src: &str, start: usize, col: usize) -> usize {
    let bytes = src.as_bytes();
    let mut pipes = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'|' && (i == 0 || bytes[i - 1] != b'\\') {
            pipes.push(i);
        }
    }
    let whole_row = pipes.len() > 1 || pipes.first().is_some_and(|&i| i != 0);
    let (open, close) = if whole_row {
        (pipes.get(col).copied(), pipes.get(col + 1).copied())
    } else {
        (pipes.first().copied(), Some(src.len()))
    };
    match (open, close) {
        (Some(open), Some(close)) => {
            let lo = open + 1; // just inside the opening pipe
            let hi = close.saturating_sub(1); // just inside the closing pipe
            let inside = if hi < lo {
                lo
            } else {
                (open + 2).clamp(lo, hi)
            };
            start + inside
        }
        (Some(open), None) => start + open + 1,
        _ => start,
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
    /// The end of the table node's source span, and the offset its trailing
    /// caret stop sits at — the one caret home past the last cell, held by the
    /// bottom border row's end. Typing there opens a paragraph under the table
    /// rather than joining the block; see `Doc::open_paragraph_at_block_edge`.
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
/// from [`VRow::media`] by [`media_spans`], so it survives the row reuse of
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

/// One formula as a frontend sees it: where its picture goes, and the TeX to
/// typeset for it. Two shapes, told apart by [`glyph`](MathInfo::glyph):
///
/// - An **inline atom** — `$E = mc^2$` in a line of prose — is one
///   [`Role::Math`] glyph on `row` at index `glyph`, standing for the whole
///   formula. A frontend that paints pictures in a line typesets the TeX at
///   the run's font size and draws the picture in the glyph's place, as wide
///   as the picture is and with the text baseline through it at its height —
///   a run delegate on Apple, an inline element on the web. The glyph is a
///   caret stop at the formula's start; the stop after it is the next glyph's.
/// - A **display block** — a paragraph holding nothing but `$$…$$` — is the
///   placeholder row (`∑ tex`, every glyph at the formula's start) plus the
///   blank fillers `rows_span` reserves under it, exactly a [`MediaInfo`]'s
///   shape. A frontend skips those rows and paints the picture there, centred
///   on the measure.
///
/// Either way the frontend supplies the *width*: core lays out in glyphs and
/// cannot know how wide a typeset formula is, which is why an inline atom is
/// one glyph and not a run of them. Only in a **column-wrapped** build does
/// that matter to the wrap: a terminal never asks for atoms (see
/// [`Surface::inline_pictures`]), and the web re-fits a row the picture
/// overflows.
///
/// A plain surface paints the glyphs as they are — `∑` for an atom, the
/// labelled row for a block — and needs none of this. Derived from
/// [`VRow::math`] by [`math_spans`], so it survives the row reuse of
/// [`BlockCache`] and [`build_spliced`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathInfo {
    /// The [`VisualMap::rows`] rows this formula occupies: `row..row + 1` for
    /// an atom, the placeholder row and its fillers for a block.
    pub rows_span: Range<usize>,
    /// The row the atom glyph, or the block's placeholder label, is on.
    pub row: usize,
    /// The atom's index into that row's glyphs, or `None` for a block.
    pub glyph: Option<usize>,
    /// The TeX between the delimiters, verbatim.
    pub tex: String,
    /// Display style rather than text style — see [`MathMark::display`].
    pub display: bool,
    /// The formula's source start: where a click on its picture lands the
    /// caret, and the same offset every placeholder glyph carries.
    pub src: usize,
}

/// A part of the document the frontend edits itself — a formula, or a fence of
/// a language it named — as [`crate::Doc::set_owned_regions`] asked. Every
/// owned region on the map is listed, in row order, whatever the frontend
/// draws over it; edits come back through [`crate::Doc::edit`] on `content`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionInfo {
    /// [`MATH_REGION`], [`FENCE_REGION`] and a language (`fence:mermaid`), or
    /// [`DIRECTIVE_REGION`] and a name (`directive:embed`).
    pub kind: String,
    /// An atom in a line of text — an inline formula's, or an inline
    /// directive's — rather than a block on rows of its own.
    pub inline: bool,
    /// A formula set in display style rather than text style — see
    /// [`MathMark::display`]. False for every other kind.
    pub display: bool,
    /// A leaf directive's name, attributes and label, as its
    /// [`DirectiveInfo`] carries them; `None` for every other kind.
    pub directive: Option<DirectiveMark>,
    /// The rows it stands on: the atom's row, or a block's placeholder row and
    /// any fillers under it.
    pub rows_span: Range<usize>,
    /// Its source start, where the caret rests in front of it.
    pub src: usize,
    /// Its source end, past its closing delimiter or fence, where the caret
    /// rests behind it.
    pub end: usize,
    /// The bytes its editor reads and splices: a formula's TeX, a fence's
    /// lines between the fences, each with its newline.
    pub content: Range<usize>,
}

/// Collect one [`RegionInfo`] per owned formula and per [`VRow::region`]
/// mark, in row order — the peer of [`math_spans`], from the final rows for the
/// same reason.
pub(super) fn region_spans(rows: &[VRow]) -> Vec<RegionInfo> {
    let mut all = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        for m in row.math.iter().filter(|m| m.owned) {
            let src = math_src(row, m);
            all.push(RegionInfo {
                kind: MATH_REGION.to_string(),
                inline: m.glyph.is_some(),
                display: m.display,
                directive: None,
                rows_span: i..i + m.rows.max(1),
                src,
                end: src + m.len,
                content: src + m.tex_at..src + m.tex_at + m.tex.len(),
            });
        }
        for m in &row.inline_regions {
            let src = row.glyphs.get(m.glyph).map_or(row.end_src, |g| g.src);
            all.push(RegionInfo {
                kind: m.kind.clone(),
                inline: true,
                display: false,
                directive: Some(m.directive.clone()),
                rows_span: i..i + 1,
                src,
                end: src + m.len,
                content: src..src + m.len,
            });
        }
        if let Some(mark) = &row.region {
            let src = row
                .glyphs
                .iter()
                .find(|g| g.ch == REGION_ATOM || g.ch == DIRECTIVE_ATOM)
                .map_or(row.end_src, |g| g.src);
            // A directive stands on as many rows as a terminal reserved for it.
            let rows = row.leaf_directive.as_ref().map_or(1, |d| d.rows.max(1));
            all.push(RegionInfo {
                kind: mark.kind.clone(),
                inline: false,
                display: false,
                directive: row.leaf_directive.clone(),
                rows_span: i..i + rows,
                src,
                end: src + mark.len,
                content: src + mark.content_at..src + mark.content_at + mark.content_len,
            });
        }
    }
    all
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

    /// What [`crate::Doc::set_directive_rows`] knows this directive by — the
    /// key a terminal reports the height of its drawing under.
    pub fn key(&self) -> DirectiveKey {
        DirectiveKey::new(&self.name, &self.label, &self.attrs)
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
        if self.destination.is_empty()
            && let Some(url) = self
                .sources
                .iter()
                .find_map(|s| first_srcset_url(&s.srcset))
        {
            return url;
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
pub(super) fn media_matches(media: &str, scheme: ColorScheme) -> bool {
    let media = media.trim();
    if media.is_empty() {
        return true;
    }
    let lower = media.to_ascii_lowercase();
    let Some(after) = lower
        .split_once("prefers-color-scheme")
        .map(|(_, rest)| rest)
    else {
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
pub(super) fn first_srcset_url(srcset: &str) -> Option<&str> {
    let first = srcset.split(',').next()?.trim();
    first.split_whitespace().next().filter(|u| !u.is_empty())
}
