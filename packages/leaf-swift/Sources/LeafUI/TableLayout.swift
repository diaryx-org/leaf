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

/// One laid-out cell: the shaped line to draw, where to draw it (aligned within
/// its column), the column's box, and the source offsets a click/caret resolves
/// against.
struct TableCellLayout {
    let line: CTLine
    let attributed: NSAttributedString
    /// Text draw origin x, relative to the table's left edge (alignment applied).
    let textX: CGFloat
    /// The column's left/right box edges, relative to the table's left edge.
    let colLeft: CGFloat
    let colRight: CGFloat
    /// Source byte offsets bounding the cell content — the caret anchors.
    let start: Int
    let end: Int
}

/// One grid row: its vertical band (relative to the table top), whether it's a
/// header row, and its cells.
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

    var width: CGFloat { colX.last ?? 0 }

    /// Lay `table` out with `theme`'s body font. `nil` when the table has no
    /// columns (nothing to draw — the caller falls back to the picture rows).
    init?(_ table: TableView, theme: EditorTheme) {
        let cols = table.grid.map { $0.cells.count }.max() ?? 0
        guard cols > 0, !table.grid.isEmpty else { return nil }

        // Shape every cell once; its width feeds the column sizing, and the line
        // is kept for drawing (no second shaping pass).
        let shaped: [[(line: CTLine, attr: NSAttributedString, width: CGFloat)]] =
            table.grid.map { row in
                row.cells.map { cell in
                    let attr = AttributedRow.makeCell(cell, head: row.head, theme: theme)
                    let line = CTLineCreateWithAttributedString(attr as CFAttributedString)
                    let w = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
                    return (line, attr, w)
                }
            }

        // Column width = its widest cell.
        var colW = [CGFloat](repeating: 0, count: cols)
        for row in shaped {
            for (c, cell) in row.enumerated() { colW[c] = max(colW[c], cell.width) }
        }

        // Column boundaries: a border, then padded content, per column.
        var boundaries = [CGFloat](repeating: 0, count: cols + 1)
        for c in 0..<cols {
            boundaries[c + 1] = boundaries[c]
                + TableMetrics.border + TableMetrics.padX + colW[c] + TableMetrics.padX
        }
        colX = boundaries

        let rowH = theme.lineHeight + 2 * TableMetrics.padY
        var laidRows: [TableRowLayout] = []
        var firstBody = table.grid.count
        var y: CGFloat = TableMetrics.border // leave the top border
        for (ri, row) in table.grid.enumerated() {
            if !row.head && firstBody == table.grid.count { firstBody = ri }
            var cells: [TableCellLayout] = []
            for c in 0..<cols where c < row.cells.count {
                let cell = row.cells[c]
                let s = shaped[ri][c]
                let contentLeft = boundaries[c] + TableMetrics.border + TableMetrics.padX
                let slack = max(0, colW[c] - s.width)
                let alignShift: CGFloat
                switch cell.align {
                case "right": alignShift = slack
                case "center": alignShift = slack / 2
                default: alignShift = 0
                }
                cells.append(TableCellLayout(
                    line: s.line,
                    attributed: s.attr,
                    textX: contentLeft + alignShift,
                    colLeft: boundaries[c],
                    colRight: boundaries[c + 1],
                    start: Int(cell.start),
                    end: Int(cell.end)
                ))
            }
            laidRows.append(TableRowLayout(top: y, height: rowH, head: row.head, cells: cells))
            y += rowH
        }
        rows = laidRows
        bodyStart = firstBody
        height = y + TableMetrics.border
    }

    /// The cell whose source range covers `src`, with its row band — for placing
    /// the caret. `nil` when no cell owns the offset.
    func cell(containing src: Int) -> (row: TableRowLayout, cell: TableCellLayout)? {
        for row in rows {
            for cell in row.cells where src >= cell.start && src <= cell.end {
                return (row, cell)
            }
        }
        return nil
    }

    /// The cell at table-relative point `(x, y)`, with its row — for a click.
    func cell(atX x: CGFloat, y: CGFloat) -> (row: TableRowLayout, cell: TableCellLayout)? {
        for row in rows where y >= row.top && y < row.top + row.height {
            for cell in row.cells where x >= cell.colLeft && x < cell.colRight {
                return (row, cell)
            }
            return row.cells.last.map { (row, $0) } // past the last column → its last cell
        }
        return nil
    }
}
