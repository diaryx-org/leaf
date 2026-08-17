//  PaginationTests.swift
//
//  The paginated flow: where a page break falls, what it is allowed to fall
//  *through*, and that the geometry queries every input path depends on — the
//  caret's rect, a point's row — still answer correctly once a row's visual lines
//  are no longer evenly spaced down from its top.
//
//  Structural invariants against the sheet's own content box rather than exact
//  pixels, so it stays font-independent like `EditorLayoutTests`. The sheet is
//  deliberately tiny (six body lines to a page) so a break happens within a
//  handful of fixture rows instead of a screenful.

import XCTest
import LeafFFI
@testable import LeafUI

final class PaginationTests: XCTestCase {
    private let theme = EditorTheme.default

    /// Six body lines of room per sheet (160pt of column at a 24pt line box), and
    /// 16pt left over — less than the 21.6pt gap that precedes a heading, which is
    /// what the collapse test leans on.
    private let page = PageSetup(
        size: CGSize(width: 400, height: 200),
        margins: LeafInsets(top: 20, left: 20, bottom: 20, right: 20),
        gap: 20, backdrop: 24)

    private let viewWidth: CGFloat = 800

    /// Long enough to wrap past one sheet's worth of lines in the test column.
    private let longText = String(repeating: "lorem ipsum dolor sit amet ", count: 30)

    private func laid(_ rows: [Row], tables: [TableView] = [],
                      paginated: Bool = true, theme: EditorTheme? = nil) -> EditorLayout {
        var cache: [Row: ShapedRow] = [:]
        return EditorLayout(docView(rows, tables: tables), theme: theme ?? self.theme,
                            viewWidth: viewWidth, page: paginated ? page : nil, cache: &cache)
    }

    /// A quote gutter run, `depth` levels deep — the shape core emits.
    private func gutter(_ depth: Int) -> Run {
        mkRun(String(repeating: "│ ", count: depth), role: "quote")
    }

    // MARK: the sheet

    func testTheColumnComesFromTheSheetsMarginsAndOverridesTheThemesMeasure() {
        // A page is a mode, not a style: it takes the text column away from
        // `measure` entirely rather than being clamped by it. An absurd measure
        // here would be visible immediately if it were still in play.
        var narrow = theme
        narrow.measure = 8
        let layout = laid([row([mkRun("hello")])], theme: narrow)
        XCTAssertEqual(layout.columnWidth, page.columnWidth)
        XCTAssertEqual(layout.originX, page.sheetX(in: viewWidth) + page.margins.left)
    }

    func testThePlaceholderCueLandsInsideTheSheetsTextBox() throws {
        // A blank paginated document opens at the first sheet's top margin, and
        // the cue has to open there with it — the case that shows most plainly
        // why the layout has to answer this rather than a host reading the
        // theme's padding, which a page has nothing to do with.
        let layout = laid([])
        let box = try XCTUnwrap(layout.placeholderBox)
        let caret = try XCTUnwrap(layout.rect(row: 0, ch: 0))
        XCTAssertEqual(box.minX, caret.minX, accuracy: 0.5)
        XCTAssertEqual(box.minY, caret.minY, accuracy: 0.5)
        XCTAssertEqual(box.minX, page.sheetX(in: viewWidth) + page.margins.left, accuracy: 0.5)
        XCTAssertEqual(box.minY, page.contentTop(0), accuracy: 0.5)
    }

    func testTheStackIsWholeSheetsHoweverLittleIsOnTheLast() {
        // One word of text still gets a whole page of paper under it — that is what
        // makes the view read as a document rather than as a scrolling column.
        let layout = laid([row([mkRun("hi")])])
        XCTAssertEqual(layout.pages.count, 1)
        XCTAssertEqual(layout.pages[0], page.sheetRect(0, x: page.sheetX(in: viewWidth)))
        XCTAssertEqual(layout.contentHeight,
                       page.sheetTop(0) + page.size.height + page.backdrop, accuracy: 0.5)
        XCTAssertEqual(layout.contentWidth, page.stackWidth)
    }

    func testTheContinuousFlowIsLeftExactlyAsItWas() {
        // The whole design rests on pagination being an *addition*: with no page
        // set, nothing is placed line-by-line and every old formula still holds.
        let layout = laid([row([mkRun(longText)])], paginated: false)
        XCTAssertTrue(layout.pages.isEmpty)
        XCTAssertEqual(layout.contentWidth, 0)
        let rl = layout.rows[0]
        XCTAssertTrue(rl.lineOrigins.isEmpty)
        XCTAssertEqual(rl.lineTop(0), rl.top, accuracy: 0.5)
        XCTAssertEqual(rl.lineTop(2), rl.top + 2 * rl.lineHeight, accuracy: 0.5)
        XCTAssertEqual(rl.bands.count, 1, "an unbroken row is one band")
        XCTAssertEqual(rl.height, CGFloat(rl.wrapped.count) * rl.lineHeight, accuracy: 0.5)
    }

    // MARK: where a break falls

    func testEveryLineLandsInsideSomeSheetsTextBox() {
        let layout = laid([row([mkRun(longText)]),
                           gapRow(.paragraph, .paragraph),
                           row([mkRun(longText)])])
        for rl in layout.rows where rl.gapHeight == nil {
            for i in rl.wrapped.indices {
                let top = rl.lineTop(i)
                let sheet = page.index(at: top)
                XCTAssertGreaterThanOrEqual(top, page.contentTop(sheet) - 0.5,
                                            "a line above sheet \(sheet)'s top margin")
                XCTAssertLessThanOrEqual(top + rl.lineHeight, page.contentBottom(sheet) + 0.5,
                                         "a line below sheet \(sheet)'s bottom margin")
            }
        }
    }

    func testALongParagraphSplitsAtTheBreakRatherThanMovingWhole() {
        let layout = laid([row([mkRun(longText)])])
        let rl = layout.rows[0]
        XCTAssertGreaterThan(rl.wrapped.count, 6, "the fixture must outrun one sheet")
        XCTAssertEqual(rl.lineTop(0), page.contentTop(0), accuracy: 0.5)

        let carried = try! XCTUnwrap(rl.wrapped.indices.first { rl.lineTop($0) >= page.sheetTop(1) })
        XCTAssertEqual(rl.lineTop(carried), page.contentTop(1), accuracy: 0.5,
                       "the carried lines head the next sheet at its top margin")
        XCTAssertEqual(rl.lineTop(carried - 1) + rl.lineHeight, page.contentBottom(0), accuracy: 24,
                       "and the ones before it fill the sheet they were on")
        XCTAssertGreaterThan(rl.bands.count, 1, "the row is drawn as one band per sheet")
    }

    func testAHeadingMovesWholeInsteadOfSplitting() {
        // Split, a heading's first line is stranded at the foot of one sheet and
        // the rest of it heads the next, which reads as two headings.
        let filler = (0..<5).map { _ in row([mkRun("x")]) }
        let heading = row([mkRun(longText)], heading: 1)
        let layout = laid(filler + [heading])
        let rl = layout.rows[5]
        XCTAssertGreaterThan(rl.wrapped.count, 1, "the fixture heading must wrap")
        XCTAssertEqual(rl.bands.count, 1, "a heading occupies one unbroken band")
        XCTAssertEqual(rl.lineTop(0), page.contentTop(1), accuracy: 0.5,
                       "and it moved whole to the next sheet")
    }

    func testAHeadingIsKeptWithTheTextItIntroduces() {
        // Five fillers leave room for the heading but not for the heading plus a
        // line of what it announces, so it goes down instead of being stranded.
        let filler = (0..<5).map { _ in row([mkRun("x")]) }
        let room = page.contentBottom(0) - (page.contentTop(0) + 5 * theme.lineHeight)
        XCTAssertLessThanOrEqual(theme.rowHeight(heading: 3), room,
                                 "the fixture must leave room for the heading alone")
        XCTAssertGreaterThan(theme.rowHeight(heading: 3) + theme.blockGap + theme.lineHeight, room,
                             "and not for it plus what follows")

        let layout = laid(filler + [row([mkRun("Title")], heading: 3),
                                    gapRow(.heading, .paragraph),
                                    row([mkRun("body")])])
        XCTAssertEqual(layout.rows[5].lineTop(0), page.contentTop(1), accuracy: 0.5,
                       "so it moved down rather than being stranded")
        XCTAssertEqual(page.index(at: layout.rows[7].lineTop(0)), 1, "and its text came with it")
    }

    func testAHeadingWithNothingAfterItIsNotPushedOntoASheetOfItsOwn() {
        // The control: keep-with-next needs a next. A heading that ends the
        // document has nothing to be kept with, and moving it would be the worse
        // answer — so in exactly the room the test above rejects, it stays.
        let filler = (0..<5).map { _ in row([mkRun("x")]) }
        let layout = laid(filler + [row([mkRun("Title")], heading: 3)])
        XCTAssertEqual(page.index(at: layout.rows[5].lineTop(0)), 0)
    }

    func testAGapThatNoLongerFitsUnderTheSheetCollapses() {
        // Paragraph spacing holds two blocks apart on one sheet. At a break there
        // is nothing left for it to separate, and carried over it would push the
        // next block an arbitrary distance below the top margin.
        let filler = (0..<6).map { _ in row([mkRun("x")]) }
        let layout = laid(filler + [gapRow(.paragraph, .heading), row([mkRun("Title")], heading: 1)])
        XCTAssertEqual(layout.rows[6].height, 0, "the gap collapsed at the break")
        XCTAssertEqual(layout.rows[7].lineTop(0), page.contentTop(1), accuracy: 0.5,
                       "so the heading sits at the next sheet's top margin, not below it")
    }

    func testAGapWithRoomForItKeepsItsHeight() {
        // The control for the case above: nothing is collapsing gaps generally.
        let layout = laid([row([mkRun("a")]), gapRow(.paragraph, .heading),
                           row([mkRun("Title")], heading: 1)])
        XCTAssertEqual(layout.rows[1].height, theme.blockGap(Boundary(above: .paragraph, below: .heading)),
                       accuracy: 0.5)
    }

    func testABlockTallerThanASheetTakesTheSheetsItCoversToItself() {
        // Nothing can make it fit, so it is placed and left to overflow — the one
        // case the break loop must not try to solve, since bouncing it to a fresh
        // sheet forever is the shape of a hang. What matters is that the walk
        // terminates and the *next* block resumes on a clean sheet below it.
        let grid = (0..<8).map { r in mkTableRow([mkCell("cell \(r)"), mkCell("value")]) }
        let picture = (0..<8).map { _ in row([mkRun("│")], decoration: true) }
        let layout = laid(picture + [row([mkRun("after")])],
                          tables: [mkTable(grid, startRow: 0, endRow: 8)])
        let table = try! XCTUnwrap(layout.rows.first { $0.tableFirst })
        XCTAssertGreaterThan(table.height, page.columnHeight, "the fixture must outrun a sheet")

        let after = layout.rows[8]
        XCTAssertGreaterThanOrEqual(after.lineTop(0), table.tableTop + table.height - 0.5,
                                    "the next block starts below the whole grid")
        XCTAssertEqual(after.lineTop(0), page.contentTop(page.index(at: after.lineTop(0))), accuracy: 0.5,
                       "and at a sheet's top margin, not part way down one")
        XCTAssertGreaterThanOrEqual(layout.pages.count, page.index(at: after.lineTop(0)) + 1,
                                    "every sheet it spilled across is drawn")
    }

    // MARK: the queries that ride the split

    func testTheCaretFollowsASplitParagraphOntoTheNextSheet() {
        let layout = laid([row([mkRun(longText)])])
        let rl = layout.rows[0]
        let carried = try! XCTUnwrap(rl.wrapped.indices.first { rl.lineTop($0) >= page.sheetTop(1) })
        let wl = rl.wrapped[carried]
        let caret = try! XCTUnwrap(layout.rect(row: 0, ch: wl.start))
        XCTAssertEqual(caret.minY, rl.lineTop(carried), accuracy: 0.5)
        XCTAssertGreaterThanOrEqual(caret.minY, page.contentTop(1) - 0.5,
                                    "the caret is on the paper, not in the gap between sheets")
    }

    func testAPointOnTheSecondSheetHitsTheLineDrawnThere() {
        // The counterpart, and the one that would break silently: `hit` used to
        // divide the offset from the row's top by the line height, which a page
        // break in the middle of the row makes meaningless.
        let layout = laid([row([mkRun(longText)])])
        let rl = layout.rows[0]
        let carried = try! XCTUnwrap(rl.wrapped.indices.first { rl.lineTop($0) >= page.sheetTop(1) })
        let wl = rl.wrapped[carried]
        let (row, ch) = layout.hit(CGPoint(x: layout.originX + 1,
                                           y: rl.lineTop(carried) + rl.lineHeight / 2))
        XCTAssertEqual(row, 0)
        XCTAssertGreaterThanOrEqual(ch, wl.start)
        XCTAssertLessThanOrEqual(ch, wl.start + wl.length)
    }

    // MARK: two columns to a sheet

    /// The same sheet in two columns: 170pt each, still six body lines tall.
    private var twoUp: PageSetup { page.columned(2, gutter: 20) }

    private func laidTwoUp(_ rows: [Row]) -> EditorLayout {
        var cache: [Row: ShapedRow] = [:]
        return EditorLayout(docView(rows), theme: theme, viewWidth: viewWidth,
                            page: twoUp, cache: &cache)
    }

    private var twoUpSheetX: CGFloat { twoUp.sheetX(in: viewWidth) }

    /// Enough to overrun one 170pt column but not two — so a split lands in the
    /// second column of the *same* sheet, which is the shape these tests are about.
    /// `longText` at this width runs to seven columns and would prove nothing
    /// specific about the first break.
    private let columnText = String(repeating: "lorem ipsum dolor sit amet ", count: 6)

    /// Every band of `rl` sits inside one column of one sheet — the invariant a
    /// gutter exists to enforce, and the one thing that must hold whatever the
    /// fixture's length turns out to be.
    private func assertBandsRespectTheColumns(_ rl: RowLayout,
                                              file: StaticString = #filePath, line: UInt = #line) {
        for band in rl.bands {
            let col = twoUp.columnIndex(atX: band.midX, sheetX: twoUpSheetX)
            let left = twoUp.columnX(col, sheetX: twoUpSheetX)
            XCTAssertEqual(band.minX, left, accuracy: 0.5, "a band off its column's left edge",
                           file: file, line: line)
            XCTAssertLessThanOrEqual(band.maxX, left + twoUp.columnWidth + 0.5,
                                     "a band running into the gutter", file: file, line: line)
            let sheet = twoUp.index(at: band.midY)
            XCTAssertGreaterThanOrEqual(band.minY, twoUp.contentTop(sheet) - 0.5,
                                        "a band above the top margin", file: file, line: line)
            XCTAssertLessThanOrEqual(band.maxY, twoUp.contentBottom(sheet) + 0.5,
                                     "a band below the bottom margin", file: file, line: line)
        }
    }

    func testTwoColumnsSplitTheTextBoxAndPayForTheGutter() {
        XCTAssertEqual(twoUp.columnWidth, (400 - 40 - 20) / 2)
        XCTAssertEqual(twoUp.columnX(0, sheetX: 0), twoUp.margins.left)
        XCTAssertEqual(twoUp.columnX(1, sheetX: 0),
                       twoUp.margins.left + twoUp.columnWidth + twoUp.columnGutter)
        // The second column's right edge lands exactly on the right margin —
        // nothing is lost or overhangs.
        XCTAssertEqual(twoUp.columnX(1, sheetX: 0) + twoUp.columnWidth,
                       twoUp.size.width - twoUp.margins.right, accuracy: 0.01)
        XCTAssertEqual(laidTwoUp([row([mkRun("hi")])]).columnWidth, twoUp.columnWidth)
    }

    func testAColumnFillsBeforeTheNextOneStartsAndBothBeforeTheNextSheet() {
        // Reading order, not two independent streams: down column one, back up to
        // the top of the sheet for column two, and only then a new sheet.
        let layout = laidTwoUp((0..<14).map { _ in row([mkRun("x")]) })
        XCTAssertEqual(layout.rows[5].lineTop(0),
                       twoUp.contentTop(0) + 5 * theme.lineHeight, accuracy: 0.5)
        XCTAssertEqual(layout.rows[5].lineOrigin(0).x,
                       twoUp.columnX(0, sheetX: twoUpSheetX), accuracy: 0.5)

        // The seventh row heads the second column — back at the sheet's top margin.
        XCTAssertEqual(layout.rows[6].lineTop(0), twoUp.contentTop(0), accuracy: 0.5)
        XCTAssertEqual(layout.rows[6].lineOrigin(0).x,
                       twoUp.columnX(1, sheetX: twoUpSheetX), accuracy: 0.5)

        // And only the thirteenth reaches a second sheet, in its first column.
        XCTAssertEqual(layout.rows[12].lineTop(0), twoUp.contentTop(1), accuracy: 0.5)
        XCTAssertEqual(layout.rows[12].lineOrigin(0).x,
                       twoUp.columnX(0, sheetX: twoUpSheetX), accuracy: 0.5)
        XCTAssertEqual(layout.pages.count, 2)
    }

    func testAParagraphSplitAtAColumnBreakMovesSidewaysAndBackUp() {
        // The case per-line *origins* exist for. A page break moves a line's y; a
        // column break moves its x too, and puts it back above where the row
        // started — which is why `top + i·lineHeight` could not have carried this.
        let layout = laidTwoUp([row([mkRun(columnText)])])
        let rl = layout.rows[0]
        XCTAssertGreaterThan(rl.wrapped.count, 6, "the fixture must outrun one column")
        XCTAssertLessThanOrEqual(rl.wrapped.count, 12, "and must not outrun two")

        let first = rl.lineOrigin(0)
        let carried = try! XCTUnwrap(rl.wrapped.indices.first {
            rl.lineOrigin($0).x > first.x + 1
        })
        XCTAssertEqual(rl.lineOrigin(carried).x,
                       twoUp.columnX(1, sheetX: twoUpSheetX), accuracy: 0.5)
        XCTAssertEqual(rl.lineOrigin(carried).y, twoUp.contentTop(0), accuracy: 0.5)
        XCTAssertLessThan(rl.lineOrigin(carried).y, rl.lineOrigin(carried - 1).y,
                          "the second column starts above where the first ended")
        XCTAssertEqual(rl.bands.count, 2, "one band per column, so nothing paints across the gutter")
        assertBandsRespectTheColumns(rl)
    }

    func testARowSpanningManyColumnsKeepsEveryBandInsideOne() {
        // The general case of the above: text long enough to run over four sheets'
        // worth of columns still never puts a band in a gutter or a margin.
        let rl = laidTwoUp([row([mkRun(longText)])]).rows[0]
        XCTAssertGreaterThan(rl.bands.count, 2)
        assertBandsRespectTheColumns(rl)
    }

    func testAPointInTheSecondColumnHitsTheLineDrawnThere() {
        // The regression the whole `locate` rewrite exists for: `rows` is no longer
        // ordered by y, so "the first row the point is above" would have answered
        // with a line from column one at the same height.
        let layout = laidTwoUp((0..<10).map { r in row([mkRun("row \(r)")]) })
        let target = layout.rows[7]
        let (hitRow, _) = layout.hit(CGPoint(x: target.lineOrigin(0).x + 4,
                                             y: target.lineTop(0) + target.lineHeight / 2))
        XCTAssertEqual(hitRow, 7)

        // And the same height in the *first* column still answers with that column.
        let (leftRow, _) = layout.hit(CGPoint(x: layout.rows[1].lineOrigin(0).x + 4,
                                              y: target.lineTop(0) + target.lineHeight / 2))
        XCTAssertEqual(leftRow, 1)
    }

    func testTheCaretRidesTheColumnItsLineLandedIn() {
        let layout = laidTwoUp((0..<10).map { r in row([mkRun("row \(r)")]) })
        let dv = docView((0..<10).map { r in row([mkRun("row \(r)")]) }, caretRow: 7, caretCh: 0)
        let caret = try! XCTUnwrap(layout.caretRect(dv, theme: theme))
        XCTAssertEqual(caret.minX, twoUp.columnX(1, sheetX: twoUpSheetX), accuracy: 0.5)
        XCTAssertEqual(caret.minY, layout.rows[7].lineTop(0), accuracy: 0.5)
    }

    func testAQuoteSplitAcrossColumnsGetsABarInEachOfThem() {
        let rl = laidTwoUp([row([gutter(1), mkRun(columnText)])]).rows[0]
        XCTAssertEqual(rl.bands.count, 2)
        let bars = rl.quoteBars(theme: theme)
        XCTAssertEqual(bars.count, 2, "one bar per column, not one spanning the gutter")
        XCTAssertNotEqual(bars[0].minX, bars[1].minX, accuracy: 1,
                          "and they sit in different columns")
        assertBandsRespectTheColumns(rl)
    }

    func testOneColumnIsExactlyTheSheetItAlwaysWas() {
        // `columns: 1` must not be a special case that drifted — it is the same
        // geometry the single-column tests above assert.
        XCTAssertEqual(page.columns, 1)
        XCTAssertEqual(page.columnX(0, sheetX: 7), 7 + page.margins.left)
        XCTAssertEqual(page.slot(3, 0), 3, "one column per sheet ⇒ a slot is a sheet")
        XCTAssertEqual(twoUp.slot(3, 1), 7)
    }

    func testAQuoteSplitAcrossSheetsGetsABarOnEachOfThem() {
        // A bar measured off the row's whole height would run over the backdrop
        // between the two sheets.
        let layout = laid([row([gutter(1), mkRun(longText)])])
        let rl = layout.rows[0]
        XCTAssertGreaterThan(rl.bands.count, 1, "the fixture must actually split")
        XCTAssertEqual(rl.quoteBars(theme: theme).count, rl.bands.count)
        for bar in rl.quoteBars(theme: theme) {
            let sheet = page.index(at: bar.minY)
            XCTAssertLessThanOrEqual(bar.maxY, page.contentBottom(sheet) + 0.5)
        }
        // And the merge across rows still reads them as separate runs.
        XCTAssertEqual(BlockChrome.quoteBarRuns([rl], theme: theme).count, rl.bands.count)
    }
}
