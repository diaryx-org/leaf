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

    func testAnEmptyHeadingRowLaysOutAtHeadingHeight() {
        // `# ` with nothing typed after it — what the toolbar's H1 leaves on a
        // blank line. Core carries the level on the row itself (an empty heading
        // has no run to read it from), so the line box, and the caret standing in
        // it, are heading-sized before the first character rather than after it.
        let empty = row([], heading: 1)
        let dv = docView([row([mkRun("a")]), empty], caretRow: 1)
        let layout = EditorLayout(dv, theme: theme)
        XCTAssertEqual(layout.rows[1].height, theme.rowHeight(heading: 1), accuracy: 0.5)
        XCTAssertGreaterThan(layout.rows[1].height, layout.rows[0].height)
        let caret = layout.caretRect(dv, theme: theme)
        XCTAssertEqual(caret?.height ?? 0, theme.rowHeight(heading: 1), accuracy: 0.5)
    }

    func testBlockGapRowIsShorterThanALine() {
        // Core spells a paragraph boundary with an empty decoration row. It must
        // lay out at the shrunk gap height, not a full line box — otherwise the
        // boundary reads as a blank line the user never typed.
        let gap = gapRow(.paragraph, .paragraph)
        let dv = docView([row([mkRun("a")]), gap, row([mkRun("b")])])
        let layout = EditorLayout(dv, theme: theme)
        let expected = theme.padding.top + theme.padding.bottom
            + theme.rowHeight(heading: nil) * 2 + theme.blockGap
        XCTAssertEqual(layout.contentHeight, expected, accuracy: 0.5)
        XCTAssertLessThan(theme.blockGap, theme.rowHeight(heading: nil))
    }

    func testDirectiveLabelReservesHeaderSpaceAboveTheRowsOwnText() {
        // Regression: the audience label used to paint directly over the first
        // row's own text (same top-left origin) instead of in its own strip.
        let labeled = row([mkRun("hello")], directive: true, directiveLabel: "public family")
        let dv = docView([labeled])
        let layout = EditorLayout(dv, theme: theme)
        let rl = layout.rows[0]
        XCTAssertEqual(rl.labelInset, theme.directiveLabelHeight)
        XCTAssertEqual(rl.height, theme.directiveLabelHeight + theme.rowHeight(heading: nil), accuracy: 0.5)
        // The row's own text draws below the label strip, not at the row's top.
        let textRect = try! XCTUnwrap(layout.rect(row: 0, ch: 0))
        XCTAssertEqual(textRect.minY, rl.top + theme.directiveLabelHeight, accuracy: 0.5)
    }

    func testUnlabeledDirectiveRowHasNoHeaderInset() {
        // Only the first (labeled) row of a directive block reserves the strip;
        // later rows in the same block sit flush, like an ordinary row.
        let unlabeled = row([mkRun("world")], directive: true, directiveLabel: nil)
        let dv = docView([unlabeled])
        let layout = EditorLayout(dv, theme: theme)
        XCTAssertEqual(layout.rows[0].labelInset, 0)
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

        let rects = layout.tableSelectionRects(from: 2, to: 4)
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
        XCTAssertTrue(layout.tableSelectionRects(from: 20, to: 30).isEmpty)
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

    func testAnEmptyDocumentStillHasALineBoxForTheCaret() throws {
        // Core publishes no rows for an empty document — it describes blocks, and
        // there is no block to describe — but the caret still has a home there, at
        // offset 0. Regression: with no row, `rect` had nothing to answer about and
        // a brand-new note drew no caret at all until the first character was typed.
        let layout = EditorLayout(docView([]), theme: theme, wrapWidth: 400)
        XCTAssertEqual(layout.rows.count, 1, "one empty line box stands in")
        let caret = try XCTUnwrap(layout.rect(row: 0, ch: 0))
        XCTAssertEqual(caret.minX, theme.padding.left, accuracy: 0.5)
        XCTAssertEqual(caret.minY, theme.padding.top, accuracy: 0.5)
        XCTAssertEqual(caret.height, theme.lineHeight, accuracy: 0.5)
        // And a tap anywhere in the blank pane lands on it.
        let (row, ch) = layout.hit(CGPoint(x: 200, y: 800))
        XCTAssertEqual(row, 0)
        XCTAssertEqual(ch, 0)
    }

    // MARK: the placeholder cue

    func testPlaceholderBoxStartsWhereTheFirstTypedCharacterWill() throws {
        // The whole point of the box: a host can't work this out from the theme,
        // because `measure` centres the column in a wide view and `padding.left`
        // is only the floor it can't cross. Regression: a cue placed at the
        // padding sat a long way left of the prose that replaced it.
        let layout = EditorLayout(docView([]), theme: theme, viewWidth: 1200)
        let box = try XCTUnwrap(layout.placeholderBox)
        let caret = try XCTUnwrap(layout.rect(row: 0, ch: 0))
        XCTAssertEqual(box.minX, caret.minX, accuracy: 0.5, "the cue's first letter is where the caret stands")
        XCTAssertEqual(box.minY, caret.minY, accuracy: 0.5)
        XCTAssertEqual(box.height, caret.height, accuracy: 0.5, "one line box, so the baselines agree")
        XCTAssertGreaterThan(box.minX, theme.padding.left + 1,
                             "a measure centres the column, so the padding is not the answer")
    }

    func testPlaceholderBoxFollowsTheColumnWhenTheresNoMeasure() throws {
        // Without a measure the column fills what the padding leaves, and the cue
        // is back at the left inset — the case a host's guess got right, kept so
        // the fix doesn't quietly move it in the other direction.
        var wide = theme
        wide.measure = nil
        let layout = EditorLayout(docView([]), theme: wide, viewWidth: 1200)
        let box = try XCTUnwrap(layout.placeholderBox)
        XCTAssertEqual(box.minX, wide.padding.left, accuracy: 0.5)
        XCTAssertEqual(box.minY, wide.padding.top, accuracy: 0.5)
    }

    func testPlaceholderBoxIsNilOnceThereIsAnythingToRead() {
        XCTAssertNil(EditorLayout(docView([row([mkRun("a")])]), theme: theme, viewWidth: 1200).placeholderBox)
        // A block the reader typed but whose glyphs a surface redraws as graphics
        // is still something they wrote: a thematic break's `───`, a table's box
        // picture. The runs carry them, so one rule covers every kind.
        let rule = row([mkRun("───", role: "rule")], decoration: true)
        XCTAssertNil(EditorLayout(docView([rule]), theme: theme, viewWidth: 1200).placeholderBox)
    }

    func testAnEmptyParagraphRowStillCountsAsAnEmptyDocument() throws {
        // Core publishes no rows for an empty document, but a document that is one
        // empty paragraph — the shape a new note takes the moment anything asks it
        // for a block — publishes a row with no glyphs. Both are blank pages, and
        // the cue belongs on both.
        let layout = EditorLayout(docView([row([])]), theme: theme, viewWidth: 1200)
        let box = try XCTUnwrap(layout.placeholderBox)
        XCTAssertEqual(box.minY, theme.padding.top, accuracy: 0.5)
    }

    func testRectIsNilForRowOutOfRange() {
        let layout = EditorLayout(docView([row([mkRun("x")])]), theme: theme)
        XCTAssertNil(layout.rect(row: 5, ch: 0))
    }

    func testCaretXAdvancesWithColumn() throws {
        let layout = EditorLayout(docView([row([mkRun("hello world")])]), theme: theme)
        let x0 = try XCTUnwrap(layout.rect(row: 0, ch: 0)).minX
        let x5 = try XCTUnwrap(layout.rect(row: 0, ch: 5)).minX
        XCTAssertGreaterThan(x5, x0)
    }

    func testHitReturnsRowFromVerticalBand() {
        let layout = EditorLayout(docView([row([mkRun("first")]), row([mkRun("second")]), row([mkRun("third")])]), theme: theme)
        let rh = theme.rowHeight(heading: nil)
        let yMidRow1 = theme.padding.top + rh * 1.5
        let (r, _) = layout.hit(CGPoint(x: theme.padding.left + 4, y: yMidRow1))
        XCTAssertEqual(r, 1)
    }

    func testHitBelowLastRowClampsToLastRow() {
        let layout = EditorLayout(docView([row([mkRun("only")]), row([mkRun("last")])]), theme: theme)
        let (r, _) = layout.hit(CGPoint(x: 10, y: 99_999))
        XCTAssertEqual(r, 1)
    }

    func testHitChIsWithinRowLength() {
        let layout = EditorLayout(docView([row([mkRun("hello")])]), theme: theme)
        let (_, ch) = layout.hit(CGPoint(x: 10_000, y: theme.padding.top + 4))
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
        let start = try XCTUnwrap(layout.rect(row: 0, ch: 0))
        // A ch just past the first wrap point sits on line 2: lower, and back near the left.
        let wrapped = try XCTUnwrap(layout.rect(row: 0, ch: firstLineEnd))
        XCTAssertGreaterThan(wrapped.minY, start.minY, "wrapped position is on a lower visual line")
    }

    func testHitOnSecondVisualLineReturnsLaterOffset() {
        let long = "the quick brown fox jumps over the lazy dog and then keeps on running"
        let layout = EditorLayout(docView([row([mkRun(long)])]), theme: theme, wrapWidth: 120)
        let lineHeight = layout.rows[0].lineHeight
        let onLine2 = CGPoint(x: theme.padding.left + 5, y: theme.padding.top + lineHeight * 1.5)
        let (r, ch) = layout.hit(onLine2)
        XCTAssertEqual(r, 0)
        XCTAssertGreaterThan(ch, 0, "a hit on the second visual line maps past the first line's text")
    }

    // MARK: the measure — a capped, centred text column

    /// A view wide enough that the measure, not the padding, decides the column.
    private let wideView: CGFloat = 2000

    func testMeasureCapsAndCentresTheColumnInAWideView() {
        let layout = EditorLayout(docView([row([mkRun("hello")])]), theme: theme, viewWidth: wideView)
        let measured = theme.measure! * theme.averageCharWidth
        XCTAssertEqual(layout.columnWidth, measured, accuracy: 0.5, "the column stops at the measure")
        XCTAssertLessThan(layout.columnWidth, wideView - theme.padding.left - theme.padding.right)
        // Centred: the room left over is split evenly, so the right margin matches
        // the left one rather than the whole surplus piling up on one side.
        let right = wideView - (layout.originX + layout.columnWidth)
        XCTAssertEqual(layout.originX, right, accuracy: 1.0, "equal margins either side")
        XCTAssertGreaterThan(layout.originX, theme.padding.left, "padding is a floor, not the origin")
    }

    func testNarrowViewShrinksToThePaddingRatherThanOverflowing() {
        // Narrower than the measure: the column is what the padding leaves, so the
        // text reflows instead of running off the edge (or scrolling sideways).
        let narrow: CGFloat = 300
        let layout = EditorLayout(docView([row([mkRun("hello")])]), theme: theme, viewWidth: narrow)
        XCTAssertEqual(layout.originX, theme.padding.left, accuracy: 0.5)
        XCTAssertEqual(layout.columnWidth, narrow - theme.padding.left - theme.padding.right, accuracy: 0.5)
    }

    func testNoMeasureFillsTheView() {
        var t = theme
        t.measure = nil
        let layout = EditorLayout(docView([row([mkRun("hello")])]), theme: t, viewWidth: wideView)
        XCTAssertEqual(layout.originX, t.padding.left, accuracy: 0.5)
        XCTAssertEqual(layout.columnWidth, wideView - t.padding.left - t.padding.right, accuracy: 0.5)
    }

    func testCaretAndHitRideTheCentredColumn() throws {
        let layout = EditorLayout(docView([row([mkRun("hello")])]), theme: theme, viewWidth: wideView)
        // Everything geometric is measured from the column, not the view's edge:
        // the caret at offset 0 sits at its left edge…
        let caret = try XCTUnwrap(layout.rect(row: 0, ch: 0))
        XCTAssertEqual(caret.minX, layout.originX, accuracy: 0.5)
        // …and a click in the left margin still lands at the start of the line,
        // rather than being measured from x=0 and landing somewhere inside it.
        let (r, ch) = layout.hit(CGPoint(x: 4, y: theme.padding.top + 2))
        XCTAssertEqual(r, 0)
        XCTAssertEqual(ch, 0)
    }

    // MARK: block boundaries — spaced by the pair core says they divide

    /// The height of the labelled gap row in a three-row `[prose, gap, prose]`
    /// frame. What the gap divides is core's answer, carried on the row, so a
    /// test states it rather than arranging neighbours for one to be inferred.
    private func gapHeight(_ above: BlockClass, _ below: BlockClass) -> CGFloat {
        let dv = docView([row([mkRun("a")]), gapRow(above, below), row([mkRun("b")])])
        return EditorLayout(dv, theme: theme, wrapWidth: 400).rows[1].height
    }

    func testHeadingTakesAWiderGapAboveThanBelow() {
        let above = gapHeight(.paragraph, .heading)
        let below = gapHeight(.heading, .paragraph)
        XCTAssertEqual(above, theme.blockGap * theme.headingGapScale, accuracy: 0.5)
        XCTAssertEqual(below, theme.blockGap, accuracy: 0.5)
        XCTAssertGreaterThan(above, below, "a heading groups with the text it introduces")
    }

    func testOrdinaryBoundariesTakeThePlainGap() {
        for boundary: (BlockClass, BlockClass) in [(.paragraph, .paragraph), (.list, .paragraph),
                                                   (.quote, .code), (.paragraph, .table)] {
            XCTAssertEqual(gapHeight(boundary.0, boundary.1), theme.blockGap, accuracy: 0.5)
        }
    }

    func testAnUnlabelledRowIsNoLongerAGapAtAll() {
        // A blank decoration row core didn't label isn't a block boundary — a
        // table's rules and a picture's reserved filler rows are decoration too.
        // It lays out as its own line box, and nothing here shrinks it.
        let dv = docView([row([mkRun("a")]), row([], decoration: true), row([mkRun("b")])])
        let rl = EditorLayout(dv, theme: theme, wrapWidth: 400).rows[1]
        XCTAssertFalse(rl.row.isBlockGap)
        XCTAssertEqual(rl.height, theme.rowHeight(heading: nil), accuracy: 0.5)
    }

    func testTheSameGapRowTakesDifferentHeightsInDifferentPlaces() {
        // The shaping cache is keyed by row *value*. Two gaps dividing different
        // pairs are now different values, so they can't collide — but the height
        // still has to come from the layout rather than the shaping, since a
        // `ShapedRow`'s line height is what the cache would hand back.
        var cache: [Row: ShapedRow] = [:]
        let layout = EditorLayout(
            docView([row([mkRun("a")]), gapRow(.paragraph, .paragraph),
                     row([mkRun("b")]), gapRow(.paragraph, .heading),
                     row([mkRun("Title")], heading: 1)]),
            theme: theme, wrapWidth: 400, cache: &cache)
        XCTAssertEqual(layout.rows[1].height, theme.blockGap, accuracy: 0.5)
        XCTAssertEqual(layout.rows[3].height, theme.blockGap * theme.headingGapScale, accuracy: 0.5)
    }

    // MARK: range rects — what a selection overlay or a find match is drawn from

    func testARangeInsideOneLineIsOneBoxWithBothEnds() throws {
        let dv = docView([row([mkRun("hello world")])])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        let boxes = layout.rangeRects(from: (0, 6), to: (0, 11))   // "world"
        XCTAssertEqual(boxes.count, 1)
        let box = try XCTUnwrap(boxes.first)
        XCTAssertTrue(box.containsStart && box.containsEnd)
        let start = try XCTUnwrap(layout.rect(row: 0, ch: 6))
        let end = try XCTUnwrap(layout.rect(row: 0, ch: 11))
        XCTAssertEqual(box.rect.minX, start.minX, accuracy: 0.5)
        XCTAssertEqual(box.rect.maxX, end.minX, accuracy: 0.5)
        XCTAssertEqual(box.rect.height, layout.rows[0].lineHeight, accuracy: 0.5)
    }

    func testARangeAcrossAWrapIsOneBoxPerVisualLine() {
        let long = "the quick brown fox jumps over the lazy dog and then keeps on running"
        let dv = docView([row([mkRun(long)])])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 160)
        let rl = layout.rows[0]
        XCTAssertGreaterThan(rl.wrapped.count, 1, "the fixture must wrap")
        let secondLine = rl.wrapped[1]
        // From two characters before the wrap to two characters into the next line.
        let boxes = layout.rangeRects(from: (0, secondLine.start - 2), to: (0, secondLine.start + 2))
        XCTAssertEqual(boxes.count, 2)
        XCTAssertTrue(boxes[0].containsStart && !boxes[0].containsEnd)
        XCTAssertTrue(!boxes[1].containsStart && boxes[1].containsEnd)
        XCTAssertEqual(boxes[0].rect.minY, rl.lineTop(0), accuracy: 0.5)
        XCTAssertEqual(boxes[1].rect.minY, rl.lineTop(1), accuracy: 0.5)
        XCTAssertEqual(boxes[1].rect.minX, rl.lineOrigin(1).x + secondLine.indent, accuracy: 0.5,
                       "the second box starts at the continuation line's own left edge")
    }

    func testARangeAcrossRowsCoversTheTailAndTheHead() {
        let dv = docView([row([mkRun("first row")]), row([mkRun("second row")])])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        let boxes = layout.rangeRects(from: (0, 6), to: (1, 6))   // "row" … "second"
        XCTAssertEqual(boxes.count, 2)
        XCTAssertEqual(boxes[0].rect.minY, layout.rows[0].top, accuracy: 0.5)
        XCTAssertEqual(boxes[1].rect.minY, layout.rows[1].top, accuracy: 0.5)
        XCTAssertEqual(boxes[1].rect.minX, layout.rows[1].originX, accuracy: 0.5, "the head starts at the margin")
        XCTAssertTrue(boxes[0].containsStart && boxes[1].containsEnd)
    }

    func testAnEmptyOrInvertedRangeHasNoBoxes() {
        let dv = docView([row([mkRun("hello")])])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        XCTAssertTrue(layout.rangeRects(from: (0, 2), to: (0, 2)).isEmpty)
        XCTAssertTrue(layout.rangeRects(from: (1, 0), to: (0, 2)).isEmpty)
    }
}
