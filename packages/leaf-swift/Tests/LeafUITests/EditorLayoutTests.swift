//  EditorLayoutTests.swift
//
//  The platform-neutral geometry engine — the map from a wrapped `DocView` to
//  laid-out rows and the two queries the views depend on (where the caret sits,
//  which (row, ch) a point hits). Asserts structural invariants, not exact pixels,
//  so it stays font-independent across machines.

import XCTest
import LeafFFI
@testable import LeafUI

final class EditorLayoutTests: XCTestCase {
    private let theme = EditorTheme.default

    func testContentHeightIsRowsPlusPadding() {
        let dv = docView([row([mkRun("hello")]), row([mkRun("world")])])
        let layout = EditorLayout(dv, theme: theme)
        let expected = theme.padding.top + theme.padding.bottom + theme.rowHeight(heading: nil) * 2
        XCTAssertEqual(layout.contentHeight, expected, accuracy: 0.5)
    }

    func testBlockGapRowIsShorterThanALine() {
        // Core spells a paragraph boundary with an empty decoration row. It must
        // lay out at the shrunk gap height, not a full line box — otherwise the
        // boundary reads as a blank line the user never typed.
        let gap = row([], decoration: true)
        let dv = docView([row([mkRun("a")]), gap, row([mkRun("b")])])
        let layout = EditorLayout(dv, theme: theme)
        let expected = theme.padding.top + theme.padding.bottom
            + theme.rowHeight(heading: nil) * 2 + theme.blockGap
        XCTAssertEqual(layout.contentHeight, expected, accuracy: 0.5)
        XCTAssertLessThan(theme.blockGap, theme.rowHeight(heading: nil))
    }

    func testTableRuleRowKeepsFullHeight() {
        // A decoration row that carries glyphs (a table's box-drawing rule) is
        // not a paragraph gap and must keep its full line box.
        let rule = row([mkRun("├────┼────┤", role: "rule")], decoration: true)
        let dv = docView([rule])
        let layout = EditorLayout(dv, theme: theme)
        let expected = theme.padding.top + theme.padding.bottom + theme.rowHeight(heading: nil)
        XCTAssertEqual(layout.contentHeight, expected, accuracy: 0.5)
    }

    func testTableLaysOutAsAGridAndCollapsesPictureRows() {
        // A 2×2 table spelled by, say, 4 picture rows collapses to one grid whose
        // height is the sum of its two grid-row bands — not the picture rows.
        let grid = mkTable([
            mkTableRow([mkCell("Feature"), mkCell("Status")], head: true),
            mkTableRow([mkCell("Tables"), mkCell("editable")]),
        ], startRow: 0, endRow: 4)
        // Four placeholder picture rows the table replaces.
        let dv = docView(
            [row([], decoration: true), row([], decoration: true),
             row([], decoration: true), row([], decoration: true)],
            tables: [grid]
        )
        let layout = EditorLayout(dv, theme: theme)
        // rows stay 1:1 with the frame (4), but only the first carries height.
        XCTAssertEqual(layout.rows.count, 4)
        XCTAssertNotNil(layout.rows[0].table)
        let rowH = theme.lineHeight + 8 // padY * 2
        let gridH = rowH * 2 + 2 // two bands + top/bottom border
        XCTAssertEqual(layout.rows[0].height, gridH, accuracy: 0.5)
        XCTAssertEqual(layout.rows[1].height, 0, "collapsed picture row")
        XCTAssertEqual(layout.contentHeight,
                       theme.padding.top + theme.padding.bottom + gridH, accuracy: 0.5)
    }

    func testTableCaretRidesTheCellItsOffsetFallsIn() throws {
        let grid = mkTable([
            mkTableRow([mkCell("ab", start: 2, end: 4), mkCell("cd", start: 7, end: 9)], head: true),
        ], startRow: 0, endRow: 2)
        // Caret at source offset 8 → inside the second cell ("cd").
        let dv = docView([row([], decoration: true), row([], decoration: true)],
                         tables: [grid], caretRow: 0, caretSrc: 8)
        let layout = EditorLayout(dv, theme: theme)
        let caret = try XCTUnwrap(layout.caretRect(dv, theme: theme))
        // Second column starts past the first, so the caret is well right of the inset.
        XCTAssertGreaterThan(caret.minX, theme.padding.left + 40)
        XCTAssertEqual(caret.height, theme.lineHeight, accuracy: 0.5)
    }

    func testMultiLineCellGrowsItsRowAndStacksTheCaret() throws {
        // A cell of two lines ("Pear" then "ripe", from a `<br>`) makes its row two
        // text-lines tall, and the caret for an offset on the second line sits a
        // line lower than one on the first.
        let grid = mkTable([
            mkTableRow([
                mkCellLines([("Pear", 2, 6), ("ripe", 11, 15)]),
                mkCell("3", start: 18, end: 19),
            ]),
        ], startRow: 0, endRow: 3)
        let dv = docView(
            [row([], decoration: true), row([], decoration: true), row([], decoration: true)],
            tables: [grid], caretRow: 0, caretSrc: 2
        )
        let layout = EditorLayout(dv, theme: theme)
        // The single grid row is two text-lines + padding tall.
        let twoLine = 2 * theme.lineHeight + 8
        XCTAssertEqual(layout.rows[0].height, twoLine + 2, accuracy: 0.5) // + top/bottom border

        // Caret on line 1 ("Pear", offset 2) vs line 2 ("ripe", offset 11): the
        // second is exactly one line lower, same height.
        let top = try XCTUnwrap(layout.caretRect(dv, theme: theme))
        let dv2 = docView(
            [row([], decoration: true), row([], decoration: true), row([], decoration: true)],
            tables: [grid], caretRow: 0, caretSrc: 11
        )
        let below = try XCTUnwrap(layout.caretRect(dv2, theme: theme))
        XCTAssertEqual(below.minY - top.minY, theme.lineHeight, accuracy: 0.5)
        XCTAssertEqual(top.height, theme.lineHeight, accuracy: 0.5)

        // The band on line 1 clears the cell's top padding (so an Up probe leaves
        // the cell), while line 2's band reaches the cell bottom.
        let band1 = try XCTUnwrap(layout.caretBand(src: 2))
        let band2 = try XCTUnwrap(layout.caretBand(src: 11))
        XCTAssertLessThan(band1.minY, top.minY, "line-1 band includes the top padding")
        XCTAssertGreaterThan(band2.maxY, below.maxY, "line-2 band reaches the bottom padding")
    }

    func testCaretBandReachesTheTablesOuterEdgesOnItsFirstAndLastRows() throws {
        // ↑ out of a table's top row probes `band.minY - 1`; if the band stops at
        // the cell (a border's-width inside the box) that probe lands on the top
        // border line, which the hit-test snaps back into the table — so the caret
        // can never reach the block above. The top row's band must reach the
        // table's true top edge (and the bottom row's its true bottom) so the
        // probe clears the box entirely.
        let grid = mkTable([
            mkTableRow([mkCell("ab", start: 2, end: 4), mkCell("cd", start: 7, end: 9)], head: true),
            mkTableRow([mkCell("ef", start: 12, end: 14), mkCell("gh", start: 17, end: 19)]),
        ], startRow: 0, endRow: 4)
        let dv = docView(
            [row([], decoration: true), row([], decoration: true),
             row([], decoration: true), row([], decoration: true)],
            tables: [grid]
        )
        let layout = EditorLayout(dv, theme: theme)
        let tableTop = layout.rows[0].tableTop
        let tableHeight = try XCTUnwrap(layout.rows[0].table).height

        // Top (header) row: the band starts at the table's outer top, so a probe
        // one point above it clears the top border.
        let topBand = try XCTUnwrap(layout.caretBand(src: 2))
        XCTAssertEqual(topBand.minY, tableTop, accuracy: 0.5, "top-row band reaches the table's top edge")
        XCTAssertLessThan(topBand.minY - 1, tableTop, "an Up probe clears the whole table")

        // Bottom (body) row: the band reaches the table's outer bottom.
        let botBand = try XCTUnwrap(layout.caretBand(src: 12))
        XCTAssertEqual(botBand.maxY, tableTop + tableHeight, accuracy: 0.5, "bottom-row band reaches the table's bottom edge")
        XCTAssertGreaterThan(botBand.maxY + 1, tableTop + tableHeight, "a Down probe clears the whole table")
    }

    func testTableSelectionCarriesIntoTheLaidOutLineAndYieldsAHighlightRect() throws {
        // A cell core marks selected carries its selected sub-range into the laid
        // out line (so the grid can paint a highlight the plain row path would
        // otherwise skip over a table), and the same range resolves to one
        // selection rect in the first band, sized to the cell text.
        let grid = mkTable([
            mkTableRow([mkSelCell("ab", start: 2, end: 4), mkCell("cd", start: 7, end: 9)], head: true),
        ], startRow: 0, endRow: 2)
        let dv = docView([row([], decoration: true), row([], decoration: true)],
                         tables: [grid], hasSelection: true)
        let layout = EditorLayout(dv, theme: theme)
        let table = try XCTUnwrap(layout.rows[0].table)
        XCTAssertFalse(table.rows[0].cells[0].lines[0].selRanges.isEmpty, "selected cell records its range")
        XCTAssertTrue(table.rows[0].cells[1].lines[0].selRanges.isEmpty, "unselected cell records none")

        let rects = layout.tableSelectionRects(from: 2, to: 4, theme: theme)
        XCTAssertEqual(rects.count, 1, "one rect for the one covered cell line")
        let r = try XCTUnwrap(rects.first)
        XCTAssertTrue(r.containsStart)
        XCTAssertTrue(r.containsEnd)
        XCTAssertGreaterThan(r.rect.width, 0, "the highlight spans the cell text")
        XCTAssertEqual(r.rect.height, theme.lineHeight, accuracy: 0.5)
        XCTAssertGreaterThan(r.rect.minX, theme.padding.left, "highlight sits inside the first column")
    }

    func testTableSelectionRectsEmptyForARangeOutsideAnyTable() {
        let grid = mkTable([
            mkTableRow([mkSelCell("ab", start: 2, end: 4)]),
        ], startRow: 0, endRow: 2)
        let dv = docView([row([], decoration: true), row([], decoration: true)], tables: [grid])
        let layout = EditorLayout(dv, theme: theme)
        XCTAssertTrue(layout.tableSelectionRects(from: 20, to: 30, theme: theme).isEmpty)
    }

    func testHeadingRowIsTaller() {
        let dv = docView([row([mkRun("Title")], heading: 1)])
        let layout = EditorLayout(dv, theme: theme)
        let expected = theme.padding.top + theme.padding.bottom + theme.rowHeight(heading: 1)
        XCTAssertEqual(layout.contentHeight, expected, accuracy: 0.5)
    }

    func testCaretRectSitsInItsRowBand() throws {
        let dv = docView([row([mkRun("alpha")]), row([mkRun("beta")])], caretRow: 1, caretCh: 2)
        let layout = EditorLayout(dv, theme: theme)
        let rect = try XCTUnwrap(layout.caretRect(dv, theme: theme))
        let rowTop = theme.padding.top + theme.rowHeight(heading: nil)
        XCTAssertEqual(rect.minY, rowTop, accuracy: 0.5)
        XCTAssertEqual(rect.height, theme.rowHeight(heading: nil), accuracy: 0.5)
        XCTAssertGreaterThan(rect.minX, theme.padding.left, "caret at ch=2 is right of the left inset")
    }

    func testRectIsNilForRowOutOfRange() {
        let layout = EditorLayout(docView([row([mkRun("x")])]), theme: theme)
        XCTAssertNil(layout.rect(row: 5, ch: 0, theme: theme))
    }

    func testCaretXAdvancesWithColumn() throws {
        let layout = EditorLayout(docView([row([mkRun("hello world")])]), theme: theme)
        let x0 = try XCTUnwrap(layout.rect(row: 0, ch: 0, theme: theme)).minX
        let x5 = try XCTUnwrap(layout.rect(row: 0, ch: 5, theme: theme)).minX
        XCTAssertGreaterThan(x5, x0)
    }

    func testHitReturnsRowFromVerticalBand() {
        let layout = EditorLayout(docView([row([mkRun("first")]), row([mkRun("second")]), row([mkRun("third")])]), theme: theme)
        let rh = theme.rowHeight(heading: nil)
        let yMidRow1 = theme.padding.top + rh * 1.5
        let (r, _) = layout.hit(CGPoint(x: theme.padding.left + 4, y: yMidRow1), theme: theme)
        XCTAssertEqual(r, 1)
    }

    func testHitBelowLastRowClampsToLastRow() {
        let layout = EditorLayout(docView([row([mkRun("only")]), row([mkRun("last")])]), theme: theme)
        let (r, _) = layout.hit(CGPoint(x: 10, y: 99_999), theme: theme)
        XCTAssertEqual(r, 1)
    }

    func testHitChIsWithinRowLength() {
        let layout = EditorLayout(docView([row([mkRun("hello")])]), theme: theme)
        let (_, ch) = layout.hit(CGPoint(x: 10_000, y: theme.padding.top + 4), theme: theme)
        XCTAssertLessThanOrEqual(ch, "hello".utf16.count, "hit clamps past end-of-line to the line length")
    }

    // MARK: incremental shaping cache

    func testCacheReusesUnchangedRowAndReshapesChangedRow() {
        var cache: [Row: ShapedRow] = [:]
        let l1 = EditorLayout(docView([row([mkRun("alpha")]), row([mkRun("beta")])]), theme: theme, wrapWidth: 400, cache: &cache)
        // Edit row 0 only; row 1 is byte-identical.
        let l2 = EditorLayout(docView([row([mkRun("alphaX")]), row([mkRun("beta")])]), theme: theme, wrapWidth: 400, cache: &cache)
        XCTAssertTrue(l1.rows[1].attributed === l2.rows[1].attributed, "unchanged row reuses its shaped text")
        XCTAssertFalse(l1.rows[0].attributed === l2.rows[0].attributed, "changed row is re-shaped")
    }

    func testCacheReuseSurvivesRowInsertion() {
        var cache: [Row: ShapedRow] = [:]
        let a = row([mkRun("a")])
        let b = row([mkRun("b")])
        let l1 = EditorLayout(docView([a, b]), theme: theme, wrapWidth: 400, cache: &cache)
        // Insert a new first row: a and b shift down one but are unchanged.
        let l2 = EditorLayout(docView([row([mkRun("new")]), a, b]), theme: theme, wrapWidth: 400, cache: &cache)
        XCTAssertTrue(l1.rows[0].attributed === l2.rows[1].attributed, "row reused despite shifting position")
        XCTAssertTrue(l1.rows[1].attributed === l2.rows[2].attributed)
    }

    func testCacheEvictsRowsNoLongerPresent() {
        var cache: [Row: ShapedRow] = [:]
        _ = EditorLayout(docView([row([mkRun("keep")]), row([mkRun("drop")])]), theme: theme, wrapWidth: 400, cache: &cache)
        _ = EditorLayout(docView([row([mkRun("keep")])]), theme: theme, wrapWidth: 400, cache: &cache)
        XCTAssertEqual(cache.count, 1, "the removed row is evicted; the cache stays bounded to the document")
        XCTAssertNotNil(cache[row([mkRun("keep")])])
    }

    func testCacheReshapesWhenWrapWidthChanges() {
        var cache: [Row: ShapedRow] = [:]
        let r = row([mkRun("the quick brown fox jumps over the lazy dog")])
        let wide = EditorLayout(docView([r]), theme: theme, wrapWidth: 4000, cache: &cache)
        let narrow = EditorLayout(docView([r]), theme: theme, wrapWidth: 80, cache: &cache)
        XCTAssertFalse(wide.rows[0].attributed === narrow.rows[0].attributed, "a resize re-shapes the row")
    }

    // MARK: pixel wrapping

    func testLongRowWrapsIntoMultipleVisualLines() {
        let long = "the quick brown fox jumps over the lazy dog and then keeps on running"
        let wide = EditorLayout(docView([row([mkRun(long)])]), theme: theme, wrapWidth: 4000)
        let narrow = EditorLayout(docView([row([mkRun(long)])]), theme: theme, wrapWidth: 120)
        XCTAssertEqual(wide.rows[0].wrapped.count, 1, "a wide budget keeps it on one line")
        XCTAssertGreaterThan(narrow.rows[0].wrapped.count, 1, "a narrow budget wraps it")
        // Content height grows with the wrapped line count.
        XCTAssertGreaterThan(narrow.contentHeight, wide.contentHeight)
    }

    func testCaretOnSecondVisualLineIsLowerAndLeftward() throws {
        let long = "the quick brown fox jumps over the lazy dog and then keeps on running"
        let layout = EditorLayout(docView([row([mkRun(long)])]), theme: theme, wrapWidth: 120)
        try XCTAssertGreaterThan(layout.rows[0].wrapped.count, 1)
        let firstLineEnd = layout.rows[0].wrapped[0].length
        let start = try XCTUnwrap(layout.rect(row: 0, ch: 0, theme: theme))
        // A ch just past the first wrap point sits on line 2: lower, and back near the left.
        let wrapped = try XCTUnwrap(layout.rect(row: 0, ch: firstLineEnd, theme: theme))
        XCTAssertGreaterThan(wrapped.minY, start.minY, "wrapped position is on a lower visual line")
    }

    func testHitOnSecondVisualLineReturnsLaterOffset() {
        let long = "the quick brown fox jumps over the lazy dog and then keeps on running"
        let layout = EditorLayout(docView([row([mkRun(long)])]), theme: theme, wrapWidth: 120)
        let lineHeight = layout.rows[0].lineHeight
        let onLine2 = CGPoint(x: theme.padding.left + 5, y: theme.padding.top + lineHeight * 1.5)
        let (r, ch) = layout.hit(onLine2, theme: theme)
        XCTAssertEqual(r, 0)
        XCTAssertGreaterThan(ch, 0, "a hit on the second visual line maps past the first line's text")
    }
}
