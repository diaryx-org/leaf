//! Tables drawn as a grid, from core's `TableInfo`.
//!
//! Core spells a table twice. Its `VisualMap` rows are the *monospace*
//! picture — `│` borders, columns a whole number of character cells — which is
//! right on a terminal and shears in any proportional font. Beside it,
//! `TableInfo` carries the structure with no opinion about geometry: each
//! cell's glyphs (every one with its source offset and style), its source
//! range and alignment, and which rows are the head. A frontend with its own
//! geometry skips the picture's rows and draws from that, as leaf-gpui and
//! leaf-swift do; this is that.
//!
//! Columns are as wide as their widest cell and only squeezed when the grid
//! will not fit, at which point cells wrap into what is left (leaf-gpui's
//! rule). Each cell is a `RowView` laid out by `text.rs` at its column's width
//! and aligned within it, so a cell is drawn, wrapped and hit-tested like any
//! other text, and a click lands on the glyph under it and that glyph's
//! source offset — never on a column of the picture.

use std::rc::Rc;

use crate::bridge::rows::{RowView, Run};
use crate::bridge::style::class_of;
use leaf_core::{Alignment as Align, Glyph, Style, TableInfo};

use crate::text::{Alignment, Fonts, Grid, Kind, RowLayout, Type, cols};

/// Space inside a cell, either side of its text, and below and above it.
pub const PAD_X: f32 = 8.0;
pub const PAD_Y: f32 = 3.0;
/// A border's width. Each cell draws its right and bottom one; the table
/// draws the left and top.
pub const BORDER: f32 = 1.0;
/// The narrowest a squeezed column's text gets.
const MIN_TEXT: f32 = 40.0;

/// A table's chrome, in CSS pixels: [`PAD_X`], [`PAD_Y`] and [`BORDER`] in a
/// window, whole cells on a grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// Space inside a cell, either side of its text.
    pub pad_x: f32,
    /// Space inside a cell, below and above its text.
    pub pad_y: f32,
    /// A column rule's width.
    pub across: f32,
    /// A row rule's height.
    pub down: f32,
    /// The grid the table is drawn on, if it is drawn on one.
    pub grid: Option<Grid>,
}

impl Rules {
    /// In a window, a hairline under every row. On a grid a rule is a cell
    /// wide or a row high, so it is drawn in a row of its own; leaf-tui's
    /// tables rule only the head and the bottom, which keeps a table of
    /// many rows from doubling in height.
    pub fn of(ty: &Type) -> Self {
        match ty.grid {
            None => Self {
                pad_x: PAD_X,
                pad_y: PAD_Y,
                across: BORDER,
                down: BORDER,
                grid: None,
            },
            Some(g) => Self {
                pad_x: g.width,
                pad_y: 0.0,
                across: g.width,
                down: g.height,
                grid: Some(g),
            },
        }
    }

    /// Whether row `r` of `rows` has a rule under it: every row in a window;
    /// on a grid only the head and the last, as core's picture of a table.
    pub fn ruled(&self, r: usize, rows: &[GridRow]) -> bool {
        self.grid.is_none() || rows[r].head || r + 1 == rows.len()
    }

    /// A text width rounded up: to a whole pixel, or on a grid to a whole
    /// cell, so that every column, and every rule after it, starts on one.
    fn up(&self, w: f32) -> f32 {
        match self.grid {
            None => w.ceil(),
            Some(g) => (w / g.width - 0.001).ceil() * g.width,
        }
    }

    /// A squeezed text width, on a grid rounded down to a whole cell.
    fn down_to(&self, w: f32) -> f32 {
        match self.grid {
            None => w,
            Some(g) => ((w / g.width + 0.001).floor() * g.width).max(g.width),
        }
    }
}

/// A table as it draws: its rows' cells, as views, and its block prefix.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TableView {
    /// A quote's gutter or a list's indent, drawn at the start of every row.
    pub prefix: RowView,
    pub rows: Vec<GridRow>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct GridRow {
    pub head: bool,
    pub cells: Vec<Cell>,
}

/// One cell: what it draws, and the way back from its text to the source.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Cell {
    pub view: RowView,
    /// Each glyph's first byte in the cell's text, and its source offset.
    pub srcs: Vec<(usize, usize)>,
    /// The cell's end in the source: where a caret after its last character
    /// rests.
    pub end: usize,
    pub align: AlignKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AlignKind {
    Start,
    Center,
    End,
}

impl From<Align> for AlignKind {
    fn from(a: Align) -> Self {
        match a {
            Align::Center => AlignKind::Center,
            Align::Right => AlignKind::End,
            Align::Default | Align::Left => AlignKind::Start,
        }
    }
}

impl From<AlignKind> for Alignment {
    fn from(a: AlignKind) -> Self {
        match a {
            AlignKind::Start => Alignment::Start,
            AlignKind::Center => Alignment::Center,
            AlignKind::End => Alignment::End,
        }
    }
}

impl Cell {
    /// The source offset at byte `at` of the cell's text: the glyph that
    /// starts there, or the cell's end past the last one.
    pub fn offset(&self, at: usize) -> usize {
        self.srcs
            .iter()
            .find(|&&(b, _)| b >= at)
            .map_or(self.end, |&(_, s)| s)
    }
}

/// The view of a table, with the caret, the selection and a composition
/// marked in its cells.
pub fn view_of(
    info: &TableInfo,
    caret: usize,
    selection: Option<(usize, usize)>,
    composing: Option<(usize, usize)>,
) -> TableView {
    let within =
        |r: Option<(usize, usize)>, at: usize| r.is_some_and(|(lo, hi)| at >= lo && at < hi);
    let runs_of = |glyphs: &[Glyph], bold: bool| -> (Vec<Run>, Vec<(usize, usize)>) {
        let mut runs: Vec<Run> = Vec::new();
        let mut srcs = Vec::with_capacity(glyphs.len());
        let mut open: Option<(Style, bool, bool)> = None;
        let mut class = String::new();
        let mut bytes = 0;
        for g in glyphs {
            let key = (g.style, within(selection, g.src), within(composing, g.src));
            if open != Some(key) {
                class.clear();
                class_of(&g.style, &mut class);
                if bold && !g.style.bold {
                    class.push_str(" b");
                }
                if key.1 {
                    class.push_str(" sel");
                }
                if key.2 {
                    class.push_str(" ime");
                }
                runs.push(Run {
                    text: String::new(),
                    class: class.clone(),
                    src: None,
                });
                open = Some(key);
            }
            srcs.push((bytes, g.src));
            let ch = g.drawn();
            runs.last_mut()
                .expect("a run was just opened")
                .text
                .push(ch);
            bytes += ch.len_utf8();
        }
        (runs, srcs)
    };
    let (prefix, _) = runs_of(&info.prefix, false);
    let rows = info
        .grid
        .iter()
        .map(|row| GridRow {
            head: row.head,
            cells: row
                .cells
                .iter()
                .map(|cell| {
                    let (runs, srcs) = runs_of(&cell.glyphs, row.head);
                    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                    // The caret is in the cell anywhere from its start to its
                    // end; at a glyph's offset it stands before that glyph.
                    let caret_col = (cell.start..=cell.end).contains(&caret).then(|| {
                        let at = srcs
                            .iter()
                            .find(|&&(_, s)| s >= caret)
                            .map_or(text.len(), |&(b, _)| b);
                        cols(&text[..at])
                    });
                    Cell {
                        view: RowView {
                            runs,
                            caret_col,
                            ..RowView::default()
                        },
                        srcs,
                        end: cell.end,
                        align: cell.align.into(),
                    }
                })
                .collect(),
        })
        .collect();
    TableView {
        prefix: RowView {
            runs: prefix,
            ..RowView::default()
        },
        rows,
    }
}

/// A table laid out: its columns' widths, and every cell's lines.
pub struct TableLayout {
    /// The prefix's width; the grid starts past it.
    pub indent: f32,
    pub prefix: RowLayout,
    /// Each column's outer width: text, padding and its right border.
    pub cols: Vec<f32>,
    pub rules: Rules,
    pub rows: Vec<RowGeom>,
    /// The whole table, borders included.
    pub height: f32,
}

pub struct RowGeom {
    /// Outer height: the tallest cell's text, padding and the bottom border,
    /// where the row has one.
    pub height: f32,
    /// Each cell's lines, where the row has that cell.
    pub cells: Vec<Option<RowLayout>>,
}

impl TableLayout {
    /// How far into the table's row column `c`'s text starts.
    pub fn text_left(&self, c: usize) -> f32 {
        self.indent + self.rules.across + self.cols[..c].iter().sum::<f32>() + self.rules.pad_x
    }
}

/// Lay a table out in `avail` pixels.
pub fn layout(fonts: &mut Fonts, view: &TableView, avail: f32) -> TableLayout {
    let prefix = fonts.layout(&view.prefix, f32::INFINITY, Kind::Prose);
    let indent = if view.prefix.runs.is_empty() {
        0.0
    } else {
        prefix.width
    };
    let n = view.rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let rules = Rules::of(fonts.ty());

    // Every column at its widest cell, unwrapped.
    let mut want = vec![0f32; n];
    for row in &view.rows {
        for (c, cell) in row.cells.iter().enumerate() {
            let w = fonts.layout(&cell.view, f32::INFINITY, Kind::Prose).width;
            want[c] = want[c].max(rules.up(w));
        }
    }
    let chrome = rules.pad_x * 2.0 + rules.across;
    let room = (avail - indent - rules.across - chrome * n as f32).max(MIN_TEXT * n as f32);
    let text: Vec<f32> = fit(&want, room)
        .into_iter()
        .map(|t| rules.down_to(t))
        .collect();

    let rows: Vec<RowGeom> = view
        .rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            let cells: Vec<Option<RowLayout>> = (0..n)
                .map(|c| {
                    row.cells.get(c).map(|cell| {
                        fonts.layout_aligned(&cell.view, text[c], Kind::Prose, cell.align.into())
                    })
                })
                .collect();
            let tallest = cells.iter().flatten().map(|l| l.height).fold(0.0, f32::max);
            let rule = if rules.ruled(r, &view.rows) {
                rules.down
            } else {
                0.0
            };
            RowGeom {
                height: tallest + rules.pad_y * 2.0 + rule,
                cells,
            }
        })
        .collect();
    let height = rules.down + rows.iter().map(|r| r.height).sum::<f32>();
    TableLayout {
        indent,
        prefix,
        cols: text.iter().map(|t| t + chrome).collect(),
        rules,
        rows,
        height,
    }
}

/// Text widths for columns that want `want` in `room`: as wanted if they
/// fit; otherwise the narrow ones keep theirs and the wide share what is
/// left in proportion, none below `MIN_TEXT`.
fn fit(want: &[f32], room: f32) -> Vec<f32> {
    let total: f32 = want.iter().sum();
    if total <= room || want.is_empty() {
        return want
            .iter()
            .map(|w| w.max(MIN_TEXT.min(*w).max(1.0)))
            .collect();
    }
    let mut out = want.to_vec();
    let mut fixed = vec![false; want.len()];
    // Columns under an even share keep their width; repeat until none
    // further qualifies, then divide the rest by what the wide ones wanted.
    loop {
        let free: Vec<usize> = (0..want.len()).filter(|&c| !fixed[c]).collect();
        let used: f32 = (0..want.len()).filter(|&c| fixed[c]).map(|c| out[c]).sum();
        let share = (room - used) / free.len().max(1) as f32;
        let narrow: Vec<usize> = free.iter().copied().filter(|&c| want[c] <= share).collect();
        if narrow.is_empty() {
            let wanted: f32 = free.iter().map(|&c| want[c]).sum();
            for &c in &free {
                out[c] = ((room - used) * want[c] / wanted).max(MIN_TEXT);
            }
            return out;
        }
        for c in narrow {
            fixed[c] = true;
        }
    }
}

/// A table shared between the frame and the render: the row it starts at,
/// what it draws, and how.
#[derive(Clone)]
pub struct Block {
    pub first: usize,
    pub rows: std::ops::Range<usize>,
    pub view: Rc<TableView>,
    pub layout: Rc<TableLayout>,
    /// The column width the layout was made for.
    pub layout_width: f32,
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        self.first == other.first
            && Rc::ptr_eq(&self.view, &other.view)
            && Rc::ptr_eq(&self.layout, &other.layout)
    }
}

#[cfg(test)]
mod tests {
    use leaf_core::{Doc, Format};

    use super::*;

    fn doc(src: &str) -> Doc {
        let mut d = Doc::from_source(src.into(), Format::Markdown).expect("a document");
        d.build_visual_unwrapped();
        d
    }

    const TABLE: &str =
        "| Name | Notes |\n|:---|---:|\n| one | a short note |\n| two | another |\n";

    fn text(v: &RowView) -> String {
        v.runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn the_grid_is_the_cells_with_the_head_bold() {
        let d = doc(TABLE);
        let info = &d.vmap.tables[0];
        let v = view_of(info, usize::MAX, None, None);
        assert_eq!(v.rows.len(), 3);
        assert!(v.rows[0].head);
        assert_eq!(text(&v.rows[0].cells[0].view), "Name");
        assert!(v.rows[0].cells[0].view.runs[0].class.contains(" b"));
        assert_eq!(text(&v.rows[1].cells[1].view), "a short note");
        assert_eq!(v.rows[1].cells[1].align, AlignKind::End);
    }

    #[test]
    fn a_cells_text_maps_back_to_its_source() {
        let d = doc(TABLE);
        let info = &d.vmap.tables[0];
        let v = view_of(info, usize::MAX, None, None);
        let cell = &v.rows[1].cells[0];
        let one = TABLE.find("one").unwrap();
        assert_eq!(cell.offset(0), one);
        assert_eq!(cell.offset(2), one + 2);
        assert_eq!(
            cell.offset(3),
            cell.end,
            "past the last glyph is the cell's end"
        );
    }

    #[test]
    fn the_caret_is_drawn_in_its_cell() {
        let d = doc(TABLE);
        let info = &d.vmap.tables[0];
        let two = TABLE.find("two").unwrap();
        let v = view_of(info, two + 1, None, None);
        assert_eq!(v.rows[2].cells[0].view.caret_col, Some(1));
        assert_eq!(v.rows[1].cells[0].view.caret_col, None);
    }

    #[test]
    fn columns_fit_their_widest_cell_and_squeeze_when_they_must() {
        assert_eq!(fit(&[50.0, 80.0], 500.0), [50.0, 80.0]);
        let squeezed = fit(&[50.0, 900.0], 400.0);
        assert_eq!(squeezed[0], 50.0, "a narrow column keeps its width");
        assert!((squeezed[0] + squeezed[1] - 400.0).abs() < 0.5);
    }

    #[test]
    fn a_table_lays_out_taller_than_its_rows_of_text() {
        let d = doc(TABLE);
        let v = view_of(&d.vmap.tables[0], usize::MAX, None, None);
        let l = layout(&mut Fonts::new(crate::text::Type::default()), &v, 600.0);
        assert_eq!(l.cols.len(), 2);
        assert_eq!(l.rows.len(), 3);
        assert!(l.height > 3.0 * crate::viewport::LINE as f32);
    }

    #[test]
    fn a_narrow_right_aligned_cell_is_hit_where_it_is_drawn() {
        let d = doc("| Qty |\n|--:|\n| 12 |\n");
        let v = view_of(&d.vmap.tables[0], usize::MAX, None, None);
        let l = layout(&mut Fonts::new(Type::default()), &v, 600.0);
        let cell = l.rows[1].cells[0].as_ref().unwrap();
        let text = l.cols[0] - 2.0 * l.rules.pad_x - l.rules.across;
        // The text's last pixel, at the right of its column: after the `1`.
        assert_eq!(cell.col_at(0, text - 1.0), 2);
    }

    #[test]
    fn on_a_grid_a_table_is_whole_cells_ruled_under_its_head_and_last_row() {
        let g = Grid {
            width: 9.6,
            height: 24.0,
        };
        let ty = Type {
            body: "Menlo, monospace".into(),
            mono: "Menlo, monospace".into(),
            grid: Some(g),
            ..Type::default()
        };
        let d = doc("| Name | Notes |\n|---|---|\n| one | a |\n| two | b |\n| six | c |\n");
        let v = view_of(&d.vmap.tables[0], usize::MAX, None, None);
        let l = layout(&mut Fonts::new(ty), &v, 600.0);
        let cells = |w: f32| w / g.width;
        for (c, w) in l.cols.iter().enumerate() {
            assert!(
                (cells(*w) - cells(*w).round()).abs() < 0.01,
                "column {c} is {w}px"
            );
        }
        // The head: a line and its rule; the body's rows a line each, and
        // the last its rule too; the table's top rule above them all.
        let heights: Vec<f32> = l.rows.iter().map(|r| r.height).collect();
        assert_eq!(heights, [48.0, 24.0, 24.0, 48.0]);
        assert_eq!(l.height, 24.0 * 7.0);
        assert!(
            (l.text_left(1) - l.cols[0] - 2.0 * g.width).abs() < 0.01,
            "past the left rule, the first column and its padding"
        );
    }
}
