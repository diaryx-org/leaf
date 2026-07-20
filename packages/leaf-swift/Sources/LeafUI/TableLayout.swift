//  TableLayout.swift
//
//  The proportional grid a table is drawn as — the Apple peer of leaf-gpui's
//  `layout_table`. Core hands back a table two ways: the monospace box-glyph
//  picture in `DocView.rows` (right on a terminal, sheared in a proportional
//  font), and the structural `TableView` (cells, alignment, which rows are the
//  head). This lays the structural form out — columns sized to their widest
//  cell, cells padded and aligned, a header ruled off from the body — so the
//  view can paint a real grid and skip the picture rows entirely.
//
//  Single line per cell: table cells are short, and not wrapping keeps the
//  geometry (and the caret/hit math over it) simple. A cell that overflows its
//  column is clipped by the column width, not reflowed.

import CoreGraphics
import CoreText
import Foundation
import LeafFFI

enum TableMetrics {
    static let border: CGFloat = 1      // grid line thickness
    static let padX: CGFloat = 8        // cell horizontal padding
    static let padY: CGFloat = 4        // cell vertical padding
}

/// One shaped line of a cell: the `CTLine` to draw, where to draw it (aligned
/// within its column), and the source offsets a click/caret over *this line*
/// resolves against. A cell is one line unless an in-cell `<br>` splits it.
struct TableCellLineLayout {
    let line: CTLine
    let attributed: NSAttributedString
    /// Text draw origin x, relative to the table's left edge (alignment applied).
    let textX: CGFloat
    /// The UTF-16 sub-ranges of this line that lie in the active selection —
    /// filled behind the text as a selection background. Empty when nothing here
    /// is selected. Line-local (offsets into the line's own shaped string), so a
    /// `CTLineGetOffsetForStringIndex` maps them straight to draw x's.
    let selRanges: [(start: Int, end: Int)]
    /// Source offsets bounding this line — its caret home and its end stop.
    let start: Int
    let end: Int
}

/// One laid-out cell: its shaped lines, the column's box, and the source offsets
/// bounding the whole cell (a click/caret resolves against the lines).
struct TableCellLayout {
    let lines: [TableCellLineLayout]
    /// The column's left/right box edges, relative to the table's left edge.
    let colLeft: CGFloat
    let colRight: CGFloat
    /// Source byte offsets bounding the whole cell — the caret anchors.
    let start: Int
    let end: Int
}

/// One grid row: its vertical band (relative to the table top), whether it's a
/// header row, and its cells. A row is as tall as its tallest cell, so a cell
/// with an in-cell break makes the whole row grow.
struct TableRowLayout {
    let top: CGFloat
    let height: CGFloat
    let head: Bool
    let cells: [TableCellLayout]
}

/// A whole table laid out as a grid: column boundaries, row bands, and the total
/// height it occupies in the document.
struct TableLayout {
    /// Column boundary x's (count = cols + 1), relative to the table's left edge —
    /// the centres of the vertical rules.
    let colX: [CGFloat]
    let rows: [TableRowLayout]
    /// The grid-row index of the first body row (header rows precede it) — where
    /// the heavier header rule is drawn.
    let bodyStart: Int
    /// The table's total height (top border → bottom border).
    let height: CGFloat
    /// The height of one text line — a multi-line cell stacks these.
    let lineHeight: CGFloat

    var width: CGFloat { colX.last ?? 0 }

    /// Lay `table` out with `theme`'s body font. `nil` when the table has no
    /// columns (nothing to draw — the caller falls back to the picture rows).
    init?(_ table: TableView, theme: EditorTheme) {
        let cols = table.grid.map { $0.cells.count }.max() ?? 0
        guard cols > 0, !table.grid.isEmpty else { return nil }

        // Shape every cell's every line once; the widest line feeds column
        // sizing and the shaped lines are kept for drawing (no second pass).
        struct Shaped { let line: CTLine; let attr: NSAttributedString; let width: CGFloat; let sel: [(Int, Int)]; let start: Int; let end: Int }
        let shaped: [[[Shaped]]] = table.grid.map { row in
            row.cells.map { cell in
                cell.lines.map { ln in
                    let attr = AttributedRow.makeCellLine(ln, head: row.head, theme: theme)
                    let line = CTLineCreateWithAttributedString(attr as CFAttributedString)
                    let w = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
                    return Shaped(line: line, attr: attr, width: w,
                                  sel: Self.selectedRanges(ln.runs), start: Int(ln.start), end: Int(ln.end))
                }
            }
        }

        // Column width = its widest line across every cell.
        var colW = [CGFloat](repeating: 0, count: cols)
        for row in shaped {
            for (c, cell) in row.enumerated() {
                for ln in cell { colW[c] = max(colW[c], ln.width) }
            }
        }

        // Column boundaries: a border, then padded content, per column.
        var boundaries = [CGFloat](repeating: 0, count: cols + 1)
        for c in 0..<cols {
            boundaries[c + 1] = boundaries[c]
                + TableMetrics.border + TableMetrics.padX + colW[c] + TableMetrics.padX
        }
        colX = boundaries

        let lineH = theme.lineHeight
        var laidRows: [TableRowLayout] = []
        var firstBody = table.grid.count
        var y: CGFloat = TableMetrics.border // leave the top border
        for (ri, row) in table.grid.enumerated() {
            if !row.head && firstBody == table.grid.count { firstBody = ri }
            // The row is as tall as its tallest cell — an in-cell break grows it.
            let maxLines = shaped[ri].map(\.count).max() ?? 1
            let rowH = CGFloat(maxLines) * lineH + 2 * TableMetrics.padY
            var cells: [TableCellLayout] = []
            for c in 0..<cols where c < row.cells.count {
                let contentLeft = boundaries[c] + TableMetrics.border + TableMetrics.padX
                let align = row.cells[c].align
                let laidLines: [TableCellLineLayout] = shaped[ri][c].map { s in
                    let slack = max(0, colW[c] - s.width)
                    let alignShift: CGFloat
                    switch align {
                    case "right": alignShift = slack
                    case "center": alignShift = slack / 2
                    default: alignShift = 0
                    }
                    return TableCellLineLayout(
                        line: s.line, attributed: s.attr,
                        textX: contentLeft + alignShift, selRanges: s.sel, start: s.start, end: s.end
                    )
                }
                cells.append(TableCellLayout(
                    lines: laidLines,
                    colLeft: boundaries[c],
                    colRight: boundaries[c + 1],
                    start: Int(row.cells[c].start),
                    end: Int(row.cells[c].end)
                ))
            }
            laidRows.append(TableRowLayout(top: y, height: rowH, head: row.head, cells: cells))
            y += rowH
        }
        rows = laidRows
        bodyStart = firstBody
        height = y + TableMetrics.border
        lineHeight = lineH
    }

    /// Coalesce a cell line's runs into the UTF-16 ranges the active selection
    /// covers — the table peer of `EditorLayout.fillSelection`'s run walk. Core
    /// already marks each run's `sel`; adjacent selected runs merge into one
    /// range so the fill is one rect per span, not one per style change.
    private static func selectedRanges(_ runs: [Run]) -> [(Int, Int)] {
        var ranges: [(Int, Int)] = []
        var utf16 = 0
        for run in runs {
            let len = run.text.utf16.count
            if run.sel {
                if let last = ranges.last, last.1 == utf16 {
                    ranges[ranges.count - 1].1 = utf16 + len
                } else {
                    ranges.append((utf16, utf16 + len))
                }
            }
            utf16 += len
        }
        return ranges
    }

    /// The cell and the specific line whose source range covers `src`, with its
    /// row — for placing the caret. `nil` when no cell owns the offset.
    func locate(src: Int) -> (row: TableRowLayout, cell: TableCellLayout, line: TableCellLineLayout, lineIndex: Int)? {
        for row in rows {
            for cell in row.cells where src >= cell.start && src <= cell.end {
                for (i, ln) in cell.lines.enumerated() where src >= ln.start && src <= ln.end {
                    return (row, cell, ln, i)
                }
                // The offset fell in the gap a `<br>`'s bytes occupy (no stop
                // there); park on the nearest real line rather than nowhere.
                if let last = cell.lines.last {
                    return (row, cell, last, cell.lines.count - 1)
                }
            }
        }
        return nil
    }

    /// The cell and line at table-relative point `(x, y)`, with its row — for a
    /// click. The line is picked by the y offset within the row's band.
    func locate(atX x: CGFloat, y: CGFloat) -> (row: TableRowLayout, cell: TableCellLayout, line: TableCellLineLayout, lineIndex: Int)? {
        for row in rows where y >= row.top && y < row.top + row.height {
            let raw = Int((y - row.top - TableMetrics.padY) / lineHeight)
            let pick = { (cell: TableCellLayout) -> (TableCellLayout, TableCellLineLayout, Int) in
                let i = min(max(0, raw), cell.lines.count - 1)
                return (cell, cell.lines[i], i)
            }
            for cell in row.cells where x >= cell.colLeft && x < cell.colRight {
                let (c, l, i) = pick(cell)
                return (row, c, l, i)
            }
            if let last = row.cells.last { // past the last column → its last cell
                let (c, l, i) = pick(last)
                return (row, c, l, i)
            }
        }
        return nil
    }
}
