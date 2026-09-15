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
//  Columns are sized to content — each as wide as its widest cell — and only
//  squeezed when the grid won't fit the text column, at which point the cells
//  that overrun wrap into what their column has left. The same rule as the
//  other two frontends (`fit_widths` in core, `fit_widths_px` in gpui), so a
//  wide table reads the same everywhere: a grid the width of the column, never
//  one that runs off the right edge of the view.

import CoreGraphics
import CoreText
import Foundation
import LeafFFI

enum TableMetrics {
    static let border: CGFloat = 1      // grid line thickness
    static let padX: CGFloat = 8        // cell horizontal padding
    static let padY: CGFloat = 4        // cell vertical padding
    /// The narrowest a column is squeezed to when the grid won't fit. Below this
    /// a column stops carrying words and shreds them a letter per line, which
    /// reads worse than a grid that runs wide. leaf-gpui's `MIN_COL_PX`.
    static let minColumnWidth: CGFloat = 24
}

/// One shaped line of a cell: the `CTLine` to draw, where to draw it (aligned
/// within its column), and the source offsets a click/caret over *this line*
/// resolves against. A cell is one line unless an in-cell `<br>` splits it, or
/// its text overruns a squeezed column and soft-wraps into several.
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
    /// Whether this line ends at a soft wrap rather than at the cell's end or an
    /// in-cell `<br>`. Its `end` is then the next line's `start`, and an offset
    /// there belongs to the *following* line — the rule a paragraph's wrapped
    /// lines follow too (`EditorLayout.rect(row:ch:)`).
    var softWrapped: Bool = false
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

    /// Lay `table` out with `theme`'s body font into `availableWidth` points —
    /// the text column, which the grid is squeezed to fit; `<= 0` means "don't
    /// fit" (every column at its widest cell), the state before a view knows its
    /// bounds, and what an unwrapped layout asks for. `nil` when the table has no
    /// columns (nothing to draw — the caller falls back to the picture rows).
    init?(_ table: TableView, theme: EditorTheme, availableWidth: CGFloat = 0) {
        let cols = table.grid.map { $0.cells.count }.max() ?? 0
        guard cols > 0, !table.grid.isEmpty else { return nil }

        // Shape every cell's every line once, unwrapped: the widest line is each
        // column's wish, and a line that fits its column is kept as shaped (no
        // second pass to find out that it doesn't wrap).
        struct Shaped {
            let line: CTLine; let attr: NSAttributedString; let width: CGFloat
            let sel: [(Int, Int)]; let start: Int; let end: Int; var soft = false
        }
        let unwrapped: [[[Shaped]]] = table.grid.map { row in
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

        // Column width = its widest line across every cell — the wish; `fit` is
        // what the text column can actually give.
        var colW = [CGFloat](repeating: 0, count: cols)
        for row in unwrapped {
            for (c, cell) in row.enumerated() {
                for ln in cell { colW[c] = max(colW[c], ln.width) }
            }
        }
        if availableWidth > 0 { Self.fit(&colW, avail: availableWidth) }

        // A line that overruns its squeezed column wraps into it; each piece
        // carries the source offsets a caret over it resolves against.
        let shaped: [[[Shaped]]] = unwrapped.map { row in
            row.enumerated().map { c, cell in
                cell.flatMap { s -> [Shaped] in
                    guard s.width > colW[c] else { return [s] }
                    let pieces = EditorLayout.wrap(s.attr, width: colW[c])
                    // Where each piece starts in the source: the line's start plus
                    // the bytes of the text before it — the same byte ≈ UTF-16
                    // reading the caret and hit paths make of a cell line.
                    let text = s.attr.string as NSString
                    let starts = pieces.map { s.start + text.substring(to: $0.start).utf8.count }
                    return pieces.enumerated().map { i, wl in
                        let last = i == pieces.count - 1
                        let a = wl.start, b = wl.start + wl.length
                        let sel = s.sel.compactMap { r -> (Int, Int)? in
                            let cs = max(r.0, a), ce = min(r.1, b)
                            return cs < ce ? (cs - a, ce - a) : nil
                        }
                        return Shaped(line: wl.line, attr: wl.attributed, width: wl.width, sel: sel,
                                      start: starts[i], end: last ? s.end : starts[i + 1],
                                      soft: !last)
                    }
                }
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
            // The row is as tall as its tallest cell — an in-cell break, or a
            // wrapped line, grows it.
            let maxLines = shaped[ri].map(\.count).max() ?? 1
            let rowH = CGFloat(maxLines) * lineH + 2 * TableMetrics.padY
            var cells: [TableCellLayout] = []
            for c in 0..<cols where c < row.cells.count {
                let contentLeft = boundaries[c] + TableMetrics.border + TableMetrics.padX
                let align = row.cells[c].align
                let laidLines: [TableCellLineLayout] = shaped[ri][c].map { s in
                    // A wrapped line keeps the space it broke at; it is not text
                    // to align against, or a right-aligned column would sit a
                    // space short of its edge.
                    let trailing = CGFloat(CTLineGetTrailingWhitespaceWidth(s.line))
                    let slack = max(0, colW[c] - (s.width - trailing))
                    let alignShift: CGFloat
                    switch align {
                    case "right": alignShift = slack
                    case "center": alignShift = slack / 2
                    default: alignShift = 0
                    }
                    return TableCellLineLayout(
                        line: s.line, attributed: s.attr,
                        textX: contentLeft + alignShift, selRanges: s.sel, start: s.start, end: s.end,
                        softWrapped: s.soft
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

    /// Shrink `widths` until the grid fits `avail`, taking from the widest column
    /// each time so the loss is shared rather than falling on whichever column is
    /// last. No column goes below `TableMetrics.minColumnWidth`; a table with more
    /// columns than the surface has room for still overflows, which is the honest
    /// outcome — there's nothing left to give. The pixel counterpart of
    /// leaf-core's `fit_widths`, and the same walk as leaf-gpui's `fit_widths_px`.
    static func fit(_ widths: inout [CGFloat], avail: CGFloat) {
        // Chrome: every column carries a border and a gutter either side, and one
        // more border closes the grid.
        let chrome = CGFloat(widths.count) * (TableMetrics.border + 2 * TableMetrics.padX)
            + TableMetrics.border
        let budget = max(0, avail - chrome)
        let floor = TableMetrics.minColumnWidth
        // A whole point at a time: the widths are a few hundred at most, and
        // stepping keeps the shrink hitting the widest column rather than scaling
        // every column by a ratio (which would squeeze a narrow column that was
        // already fine).
        while widths.reduce(0, +) > budget {
            guard let i = widths.indices
                .filter({ widths[$0] > floor })
                .max(by: { widths[$0] < widths[$1] })
            else { return }
            // Clamped, not just decremented: real widths are fractional, so a
            // column at 24.5 would step straight through a 24.0 floor.
            widths[i] = max(floor, widths[i] - 1)
        }
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
                // A soft-wrapped line's end is the next line's start, and belongs
                // to it; a `<br>` line's end is its own stop, past its last glyph.
                for (i, ln) in cell.lines.enumerated()
                where src >= ln.start && (ln.softWrapped ? src < ln.end : src <= ln.end) {
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
