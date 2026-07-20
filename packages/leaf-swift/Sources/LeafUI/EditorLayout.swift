//  EditorLayout.swift
//
//  The platform-neutral geometry of a rendered frame. In the proportional GUI,
//  core hands back an *unwrapped* `DocView` — one `Row` per block (hard breaks
//  still split; soft wrapping is ours) — and this wraps each row to the view's
//  pixel width with Core Text, into a stack of visual lines. It answers the
//  geometry both the AppKit and UIKit views need: where the caret sits, which
//  `(row, ch)` a point hits, and how tall the content is. All Core Text +
//  Foundation, so it compiles once for both toolkits.

import CoreGraphics
import CoreText
import Foundation
import LeafFFI

/// Toolbar/chrome state pushed to the host after every repaint — the subset of a
/// `DocView` a surrounding UI reflects. Platform-neutral so both views share it.
public struct EditorState: Equatable {
    public var view: String          // "wysiwyg" | "source"
    public var dirty: Bool
    public var heading: UInt32?      // heading level at the caret, or nil
    public var active: [String]      // inline marks active at the caret

    public init(view: String, dirty: Bool, heading: UInt32?, active: [String]) {
        self.view = view; self.dirty = dirty; self.heading = heading; self.active = active
    }

    /// Project a full `DocView` down to the chrome-facing state.
    public init(_ v: DocView) {
        self.init(view: v.view, dirty: v.dirty, heading: v.heading, active: v.active)
    }
}

extension Row {
    /// Whether this is the blank decoration row core spells a block boundary
    /// with — no caret home, and (unlike a table rule or a quote gutter) no
    /// visible glyphs. These are the paragraph gaps the layout draws short so a
    /// boundary reads as spacing, not an empty line.
    var isBlockGap: Bool {
        decoration && !code && runs.allSatisfy { $0.text.allSatisfy(\.isWhitespace) }
    }
}

/// One pixel-wrapped visual line within a logical row. Its `CTLine` is built over
/// the line's *substring*, so its string indices are relative to `start`; callers
/// convert with `ch - start`. `start`/`length` are UTF-16 offsets into the row.
struct WrappedLine {
    let attributed: NSAttributedString   // the substring this visual line draws
    let line: CTLine                     // geometry over that substring (indices relative to `start`)
    let start: Int                       // absolute UTF-16 offset of the line within the row
    let length: Int                      // UTF-16 length of the line
    let width: CGFloat                   // typographic width, points
}

/// The expensive, position-independent shaping of one row: its attributed string
/// and the visual lines it wrapped into at `wrapWidth`. Cached across frames keyed
/// by the row's *value*, so an edit re-shapes only the row(s) that changed —
/// everything else, including every row below an insert/delete, is reused. A cache
/// hit is only valid at the same `wrapWidth`; a resize rebuilds. (A selection-only
/// edit flips a run's `sel` and re-shapes that row, which is harmless — `sel` isn't
/// in the attributed string; the selection is filled separately.)
struct ShapedRow {
    let attributed: NSAttributedString
    let wrapped: [WrappedLine]
    let lineHeight: CGFloat
    let wrapWidth: CGFloat
}

/// One logical row (block) placed in the document: its shaping plus a top offset.
///
/// Table rows are the exception. To keep `rows` 1:1 with the frame's rows (every
/// caret/click path indexes `rows` by a core row index), a table's box-glyph
/// picture rows are kept — but each carries the laid-out `table` grid instead of
/// text, only the FIRST one (`tableFirst`) has any height, and all of them draw
/// the grid rather than their glyphs. The caret and hit-testing over a table read
/// the grid, not these rows' shaping.
struct RowLayout {
    let row: Row
    let shaped: ShapedRow
    let top: CGFloat
    /// The grid, on every picture row of a table; `nil` for an ordinary row.
    var table: TableLayout? = nil
    /// The grid's top (all of a table's rows share it — they collapse onto it).
    var tableTop: CGFloat = 0
    /// The one picture row that carries the grid's height and paints it.
    var tableFirst: Bool = false

    var attributed: NSAttributedString { shaped.attributed }
    var wrapped: [WrappedLine] { shaped.wrapped }
    var lineHeight: CGFloat { shaped.lineHeight }
    /// The block's total height — the grid's height on a table's first row, zero
    /// on its other (collapsed) rows, else one `lineHeight` per visual line.
    var height: CGFloat {
        if let t = table { return tableFirst ? t.height : 0 }
        return CGFloat(shaped.wrapped.count) * shaped.lineHeight
    }
}

/// The laid-out rows of one `DocView` plus the geometry queries over them.
struct EditorLayout {
    let rows: [RowLayout]
    /// Total content height including top+bottom padding — the view's fitting size.
    let contentHeight: CGFloat

    /// Lay out `docView`, wrapping each row to `wrapWidth` points. Reuses shaped rows
    /// from `cache` (same content *and* same width) and replaces it with the exact
    /// set this frame used, so deleted rows are evicted and the cache stays bounded.
    /// The caller must clear `cache` when the theme's geometry changes.
    /// `wrapWidth <= 0` means "don't wrap" (one visual line per row) — the state
    /// before the view knows its bounds.
    init(_ docView: DocView, theme: EditorTheme, wrapWidth: CGFloat, cache: inout [Row: ShapedRow]) {
        var layouts: [RowLayout] = []
        layouts.reserveCapacity(docView.rows.count)
        var next = Dictionary<Row, ShapedRow>(minimumCapacity: docView.rows.count)
        var y = theme.padding.top

        // A table's box-glyph picture rows are replaced by one grid element that
        // stands in for the whole `[startRow, endRow)` span.
        var tableAt: [Int: TableView] = [:]
        for t in docView.tables { tableAt[Int(t.startRow)] = t }

        // An empty stand-in shape for a table's collapsed picture rows (they draw
        // the grid, never their own glyphs).
        let emptyShape = ShapedRow(
            attributed: NSAttributedString(),
            wrapped: [WrappedLine(attributed: NSAttributedString(),
                                  line: CTLineCreateWithAttributedString(NSAttributedString()),
                                  start: 0, length: 0, width: 0)],
            lineHeight: theme.lineHeight,
            wrapWidth: wrapWidth
        )

        var i = 0
        while i < docView.rows.count {
            if let t = tableAt[i], let grid = TableLayout(t, theme: theme) {
                let tableTop = y
                // Keep every picture row (rows stay 1:1 with the frame), but
                // collapse them onto the grid: the first carries its height, the
                // rest are zero-height, and all defer drawing/caret to the grid.
                for r in Int(t.startRow)..<Int(t.endRow) where r < docView.rows.count {
                    layouts.append(RowLayout(
                        row: docView.rows[r], shaped: emptyShape, top: tableTop,
                        table: grid, tableTop: tableTop, tableFirst: r == Int(t.startRow)
                    ))
                }
                y += grid.height
                i = Int(t.endRow)
                continue
            }

            let row = docView.rows[i]
            let shaped: ShapedRow
            if let hit = cache[row] ?? next[row], hit.wrapWidth == wrapWidth {
                shaped = hit
            } else {
                let attributed = AttributedRow.make(row, theme: theme)
                shaped = ShapedRow(
                    attributed: attributed,
                    wrapped: EditorLayout.wrap(attributed, width: wrapWidth),
                    lineHeight: theme.rowHeight(for: row),
                    wrapWidth: wrapWidth
                )
            }
            next[row] = shaped
            let rl = RowLayout(row: row, shaped: shaped, top: y)
            y += rl.height
            layouts.append(rl)
            i += 1
        }
        rows = layouts
        contentHeight = y + theme.padding.bottom
        cache = next
    }

    /// Build with no cross-frame cache — every row shaped fresh. Convenience for
    /// one-off layouts and tests.
    init(_ docView: DocView, theme: EditorTheme, wrapWidth: CGFloat) {
        var scratch: [Row: ShapedRow] = [:]
        self.init(docView, theme: theme, wrapWidth: wrapWidth, cache: &scratch)
    }

    /// Build unwrapped (one visual line per row). Convenience for tests.
    init(_ docView: DocView, theme: EditorTheme) {
        self.init(docView, theme: theme, wrapWidth: 0)
    }

    /// Break `attributed` into visual lines at `width` points via Core Text. Each
    /// line owns a `CTLine` over its substring (relative indices). `width <= 0`
    /// keeps the whole row on one line; an empty row is one empty line so it still
    /// occupies a line box and holds a caret.
    static func wrap(_ attributed: NSAttributedString, width: CGFloat) -> [WrappedLine] {
        let len = attributed.length
        if len == 0 {
            return [WrappedLine(attributed: attributed, line: CTLineCreateWithAttributedString(attributed),
                                start: 0, length: 0, width: 0)]
        }
        let typesetter = CTTypesetterCreateWithAttributedString(attributed as CFAttributedString)
        var lines: [WrappedLine] = []
        var start = 0
        while start < len {
            let count: Int = width > 0
                ? max(1, CTTypesetterSuggestLineBreak(typesetter, start, Double(width)))
                : len - start
            let sub = attributed.attributedSubstring(from: NSRange(location: start, length: count))
            let line = CTLineCreateWithAttributedString(sub as CFAttributedString)
            lines.append(WrappedLine(
                attributed: sub,
                line: line,
                start: start,
                length: count,
                width: CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
            ))
            start += count
        }
        return lines
    }

    // MARK: geometry

    /// A 1.5pt-wide vertical rect at row `row`, UTF-16 offset `ch` — the geometry a
    /// caret or a selection endpoint occupies, resolved to the visual line `ch` falls
    /// on. `nil` if the row is out of range. At a soft-wrap boundary the position
    /// belongs to the *start* of the following line.
    func rect(row: Int, ch: Int, theme: EditorTheme) -> CGRect? {
        guard rows.indices.contains(row) else { return nil }
        let rl = rows[row]
        let lines = rl.wrapped
        for (i, wl) in lines.enumerated() where ch < wl.start + wl.length || i == lines.count - 1 {
            let x = CTLineGetOffsetForStringIndex(wl.line, CFIndex(max(0, ch - wl.start)), nil)
            let y = rl.top + CGFloat(i) * rl.lineHeight
            return CGRect(x: theme.padding.left + x, y: y, width: 1.5, height: rl.lineHeight)
        }
        return CGRect(x: theme.padding.left, y: rl.top, width: 1.5, height: rl.lineHeight)
    }

    /// The caret's frame — `caret_ch` (UTF-16, within its block row) mapped through
    /// the pixel wrap to a rect. `nil` if the caret row is out of range. Inside a
    /// table the caret rides the grid (by its source offset), not the collapsed
    /// picture row `caret_row` names.
    func caretRect(_ docView: DocView, theme: EditorTheme) -> CGRect? {
        let cr = Int(docView.caretRow)
        if rows.indices.contains(cr), let grid = rows[cr].table {
            return tableCaretRect(grid, tableTop: rows[cr].tableTop,
                                  caretSrc: Int(docView.caretSrc), theme: theme)
        }
        return rect(row: cr, ch: Int(docView.caretCh), theme: theme)
    }

    /// The caret's frame inside a table: the cell *line* its source offset falls
    /// on, at the x the offset maps to within that line and the y of the line's
    /// band. A multi-line cell (an in-cell `<br>`) puts later offsets lower.
    private func tableCaretRect(_ grid: TableLayout, tableTop: CGFloat, caretSrc: Int,
                                theme: EditorTheme) -> CGRect? {
        guard let (row, _, line, lineIndex) = grid.locate(src: caretSrc) else { return nil }
        // Byte offset within the line ≈ UTF-16 index (exact for ASCII text). The
        // line carries no break, so this holds even across an in-cell `<br>`.
        let idx = max(0, min(caretSrc - line.start, line.attributed.length))
        let dx = CTLineGetOffsetForStringIndex(line.line, CFIndex(idx), nil)
        return CGRect(x: theme.padding.left + line.textX + dx,
                      y: tableTop + row.top + TableMetrics.padY + CGFloat(lineIndex) * grid.lineHeight,
                      width: 1.5, height: theme.lineHeight)
    }

    /// The source offset a click at `point` resolves to when it lands in a table,
    /// else `nil` (the caller falls back to the row/ch hit path). The offset is
    /// approximate for a cell with inline markup; core snaps it to a real stop.
    func tableHitOffset(_ point: CGPoint, theme: EditorTheme) -> Int? {
        for rl in rows {
            guard let grid = rl.table, rl.tableFirst else { continue }
            let yInTable = point.y - rl.tableTop
            guard yInTable >= 0, yInTable < grid.height else { continue }
            let xInTable = point.x - theme.padding.left
            guard let (_, _, line, _) = grid.locate(atX: xInTable, y: yInTable) else { return nil }
            let rel = CTLineGetStringIndexForPosition(
                line.line, CGPoint(x: max(0, xInTable - line.textX), y: 0))
            let clamped = max(0, min(rel, line.attributed.length))
            let prefix = (line.attributed.string as NSString).substring(to: clamped)
            return line.start + prefix.utf8.count
        }
        return nil
    }

    /// The selection rectangles for the source range `[from, to)` that fall
    /// inside tables — one per cell line the range touches, in view coordinates.
    /// Empty when the range meets no table. The peer of `fillSelection` for the
    /// grid: a table's picture rows carry no `wrapped` lines, so the ordinary
    /// row-based selection walk skips right over them and the system (iOS) or the
    /// caller would otherwise draw no highlight over a table. `from`/`to` are
    /// source byte offsets; each rect flags whether it holds an endpoint.
    func tableSelectionRects(from: Int, to: Int, theme: EditorTheme)
        -> [(rect: CGRect, containsStart: Bool, containsEnd: Bool)]
    {
        guard to > from else { return [] }
        var out: [(rect: CGRect, containsStart: Bool, containsEnd: Bool)] = []
        for rl in rows {
            guard let grid = rl.table, rl.tableFirst else { continue }
            for row in grid.rows {
                for cell in row.cells {
                    for (i, line) in cell.lines.enumerated() {
                        let cs = max(from, line.start), ce = min(to, line.end)
                        guard cs < ce else { continue }
                        // Byte offset within the line ≈ UTF-16 index (exact for
                        // ASCII), the same approximation the table caret rides.
                        let sIdx = max(0, min(cs - line.start, line.attributed.length))
                        let eIdx = max(0, min(ce - line.start, line.attributed.length))
                        let x0 = CTLineGetOffsetForStringIndex(line.line, CFIndex(sIdx), nil)
                        let x1 = CTLineGetOffsetForStringIndex(line.line, CFIndex(eIdx), nil)
                        let y = rl.tableTop + row.top + TableMetrics.padY
                            + CGFloat(i) * grid.lineHeight
                        out.append((
                            CGRect(x: theme.padding.left + line.textX + x0, y: y,
                                   width: x1 - x0, height: grid.lineHeight),
                            cs == from, ce == to
                        ))
                    }
                }
            }
        }
        return out
    }

    /// The vertical band of the cell line holding source offset `src` — a full
    /// table-cell band (clearing the cell's top/bottom padding) at the cell's
    /// first/last line, and the bare line band between. A vertical probe just past
    /// this band lands on the next line, the next cell, or out of the table,
    /// whichever is adjacent. `nil` when `src` isn't in a table (the caller uses
    /// the caret/line rect, whose thin height is already the right band there).
    func caretBand(src: Int) -> (minY: CGFloat, maxY: CGFloat)? {
        for rl in rows {
            guard let grid = rl.table, rl.tableFirst,
                  let (row, cell, _, lineIndex) = grid.locate(src: src)
            else { continue }
            let top = rl.tableTop + row.top
            let lineTop = top + TableMetrics.padY + CGFloat(lineIndex) * grid.lineHeight
            let minY = lineIndex == 0 ? top : lineTop
            let maxY = lineIndex == cell.lines.count - 1 ? top + row.height : lineTop + grid.lineHeight
            return (minY, maxY)
        }
        return nil
    }

    /// Map a point (view coordinates) to core's `(row, ch)`: the block row from the
    /// vertical band it lands in, the visual line within it from the y offset, and
    /// the UTF-16 offset from Core Text's hit-test of the horizontal position.
    /// `click_ch` then clamps `ch` to a real caret stop.
    func hit(_ point: CGPoint, theme: EditorTheme) -> (row: Int, ch: Int) {
        guard !rows.isEmpty else { return (0, 0) }
        let row = rows.firstIndex { point.y < $0.top + $0.height } ?? rows.count - 1
        let rl = rows[row]
        let lines = rl.wrapped
        let li = min(max(0, Int((point.y - rl.top) / rl.lineHeight)), lines.count - 1)
        let wl = lines[li]
        let localX = point.x - theme.padding.left
        let rel = CTLineGetStringIndexForPosition(wl.line, CGPoint(x: max(0, localX), y: 0))
        let ch = wl.start + min(max(0, rel), wl.length)
        return (row, ch)
    }

    /// The visual line index within row `row` that offset `ch` sits on, and that
    /// line's `[start, end)` UTF-16 range — for visual-line motion (Home/End/↑/↓).
    /// Returns `nil` if the row is out of range.
    func visualLine(row: Int, ch: Int) -> (index: Int, start: Int, end: Int)? {
        guard rows.indices.contains(row) else { return nil }
        let lines = rows[row].wrapped
        for (i, wl) in lines.enumerated() where ch < wl.start + wl.length || i == lines.count - 1 {
            return (i, wl.start, wl.start + wl.length)
        }
        return (0, 0, 0)
    }

    /// Fill the selection background behind the runs core marked `sel`, split across
    /// the row's visual lines, into `ctx`. Core carves the selection into run
    /// boundaries; we coalesce those into ranges and clip each to a visual line.
    func fillSelection(row rl: RowLayout, padLeft: CGFloat, color: LeafColor, in ctx: CGContext) {
        var ranges: [(Int, Int)] = []
        var utf16 = 0
        for run in rl.row.runs {
            let len = run.text.utf16.count
            if run.sel {
                if let last = ranges.last, last.1 == utf16 {
                    ranges[ranges.count - 1].1 = utf16 + len       // merge adjacent runs
                } else {
                    ranges.append((utf16, utf16 + len))
                }
            }
            utf16 += len
        }
        guard !ranges.isEmpty else { return }
        ctx.setFillColor(color.cgColor)
        for (i, wl) in rl.wrapped.enumerated() {
            let lineStart = wl.start, lineEnd = wl.start + wl.length
            let y = rl.top + CGFloat(i) * rl.lineHeight
            for (s, e) in ranges {
                let cs = max(s, lineStart), ce = min(e, lineEnd)
                guard cs < ce else { continue }
                let x0 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(cs - lineStart), nil)
                let x1 = CTLineGetOffsetForStringIndex(wl.line, CFIndex(ce - lineStart), nil)
                ctx.fill(CGRect(x: padLeft + x0, y: y, width: x1 - x0, height: rl.lineHeight))
            }
        }
    }
}
