//! Display width: how many terminal cells a glyph, a cluster, or a row takes.

use super::*;

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
pub(super) struct Cluster {
    /// Index of the glyph that opens it.
    pub(super) glyph: usize,
    /// The display column it starts at.
    col: usize,
    /// How many cells it is drawn in. Zero for a cluster with no width of its
    /// own (a lone joiner), which therefore sits at no column at all.
    pub(super) cells: usize,
}

/// Walk a row's glyphs as the clusters they spell, in column order.
pub(super) fn clusters(glyphs: &[Glyph]) -> Vec<Cluster> {
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
pub(super) fn glyphs_width(glyphs: &[Glyph]) -> usize {
    clusters(glyphs).last().map_or(0, |c| c.col + c.cells)
}

/// A cell's display width — the widest of its lines, since an in-cell `\n` break
/// splits it into several. Sizes the column that must hold every line.
pub(super) fn cell_width(glyphs: &[Glyph]) -> usize {
    glyphs
        .split(|g| g.ch == '\n')
        .map(glyphs_width)
        .max()
        .unwrap_or(0)
}

/// Whether a raw inline HTML tag is a line break (`<br>`, `<br/>`, `<br />`,
/// case-insensitively) — the one tag a table cell reads as an in-cell break.
pub(super) fn is_br(text: Option<&str>) -> bool {
    let Some(t) = text else { return false };
    matches!(
        t.trim().to_ascii_lowercase().replace(' ', "").as_str(),
        "<br>" | "<br/>"
    )
}

impl VRow {
    /// The row's width in display columns — and so the column of the caret
    /// placed past its last glyph, which is the rightmost column it can occupy.
    pub(super) fn width(&self) -> usize {
        glyphs_width(&self.glyphs)
    }

    /// The display column glyph `i` is drawn at. Glyphs continuing a cluster
    /// report the column of the glyph that opened it, since that is where they
    /// are drawn; none of them is ever a stop, so no caret is placed by it.
    pub(super) fn col_of_glyph(&self, i: usize) -> usize {
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
    pub(super) fn glyph_at_col(&self, col: usize) -> Option<usize> {
        clusters(&self.glyphs)
            .into_iter()
            .find(|c| col < c.col + c.cells)
            .map(|c| c.glyph)
    }
}
