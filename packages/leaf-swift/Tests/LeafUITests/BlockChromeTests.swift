//  BlockChromeTests.swift
//
//  The block decoration the GUI draws instead of core's monospace glyphs: a
//  blockquote's gutter bars and a thematic break's line. Asserts the structural
//  facts (which rows are breaks, how many bars a level deep, that consecutive
//  quoted rows merge into one bar, that a break collapses to one line and spans
//  the column) rather than exact pixels, so it stays font-independent.

import XCTest
import LeafFFI
@testable import LeafUI

final class BlockChromeTests: XCTestCase {
    private let theme = EditorTheme.default

    /// A quote gutter run, `depth` levels deep — the shape core emits.
    private func gutter(_ depth: Int) -> Run {
        mkRun(String(repeating: "│ ", count: depth), role: "quote")
    }

    /// A thematic break row, as core spells it: the prefix then a run of dashes.
    private func ruleRow(prefix: [Run] = [], dashes: Int = 40) -> Row {
        row(prefix + [mkRun(String(repeating: "─", count: dashes), role: "rule")])
    }

    // MARK: which rows are breaks

    func testThematicBreakIsRecognised() {
        XCTAssertTrue(ruleRow().isThematicBreak)
        XCTAssertTrue(ruleRow(prefix: [gutter(1)]).isThematicBreak, "a quoted break is still a break")
    }

    func testTableRowsAreNotThematicBreaks() {
        // A table's box rules are decoration rows…
        let boxRule = row([mkRun("├───┼───┤", role: "rule")], decoration: true)
        XCTAssertFalse(boxRule.isThematicBreak)
        // …and its content rows mix rule-role separators with real cell text.
        let content = row([mkRun("│ ", role: "rule"), mkRun("cell"), mkRun(" │", role: "rule")])
        XCTAssertFalse(content.isThematicBreak)
        // Ordinary prose never is.
        XCTAssertFalse(row([mkRun("hello")]).isThematicBreak)
    }

    // MARK: the break's own layout

    func testBreakCollapsesToOneLineHoweverNarrowTheView() {
        // The dashes are dropped in favour of a drawn line, so a 40-dash rule
        // can't wrap onto a second row in a narrow column the way its glyphs would.
        let layout = EditorLayout(docView([ruleRow()]), theme: theme, wrapWidth: 80)
        XCTAssertEqual(layout.rows[0].wrapped.count, 1)
        XCTAssertEqual(layout.rows[0].height, theme.rowHeight(heading: nil), accuracy: 0.5)
    }

    func testBreakDrawsALineAcrossTheColumn() throws {
        let layout = EditorLayout(docView([ruleRow()]), theme: theme, wrapWidth: 400)
        let rl = layout.rows[0]
        let line = try XCTUnwrap(rl.ruleLine(theme: theme))
        XCTAssertEqual(line.minX, theme.padding.left, accuracy: 0.5, "starts at the text margin")
        XCTAssertEqual(line.maxX, theme.padding.left + 400, accuracy: 0.5, "runs to the right margin")
        XCTAssertEqual(line.height, theme.ruleThickness)
        XCTAssertGreaterThan(line.minY, rl.top, "sits inside the row's box")
        XCTAssertLessThan(line.maxY, rl.top + rl.height)
    }

    func testQuotedBreakIsInsetPastItsGutterAndKeepsItsBar() throws {
        let quoted = ruleRow(prefix: [gutter(1)])
        let layout = EditorLayout(docView([quoted]), theme: theme, wrapWidth: 400)
        let rl = layout.rows[0]
        let line = try XCTUnwrap(rl.ruleLine(theme: theme))
        XCTAssertGreaterThanOrEqual(line.minX, theme.padding.left + theme.quoteIndent - 0.5,
                                    "the line starts past the quote's gutter")
        XCTAssertEqual(rl.quoteBars(theme: theme).count, 1, "the bar carries on through the rule")
    }

    func testOrdinaryRowHasNoRuleLine() {
        let layout = EditorLayout(docView([row([mkRun("hello")])]), theme: theme, wrapWidth: 400)
        XCTAssertNil(layout.rows[0].ruleLine(theme: theme))
    }

    // MARK: quote bars

    func testOneBarPerNestingLevel() {
        let dv = docView([
            row([gutter(1), mkRun("one deep")]),
            row([gutter(2), mkRun("two deep")]),
        ])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        XCTAssertEqual(layout.rows[0].quoteBars(theme: theme).count, 1)
        let nested = layout.rows[1].quoteBars(theme: theme)
        XCTAssertEqual(nested.count, 2)
        XCTAssertLessThan(nested[0].minX, nested[1].minX, "the inner level's bar sits right of the outer")
        XCTAssertEqual(nested[0].minX, theme.padding.left, accuracy: 0.5, "the outer bar is at the margin")
    }

    func testGutterIndentsQuotedTextByTheThemedWidth() throws {
        // Core's `│ ` is a couple of cramped points in a proportional font; the
        // gutter run is stretched so the text clears the painted bar.
        let dv = docView([row([gutter(1), mkRun("quoted")])])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        let textStart = try XCTUnwrap(layout.rect(row: 0, ch: 2))  // just past "│ "
        XCTAssertEqual(textStart.minX, theme.padding.left + theme.quoteIndent, accuracy: 1.0)
    }

    func testConsecutiveQuotedRowsMergeIntoOneBar() {
        let dv = docView([
            row([gutter(1), mkRun("first")]),
            row([gutter(1), mkRun("second")]),
            row([gutter(1), mkRun("third")]),
        ])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        let runs = BlockChrome.quoteBarRuns(layout.rows, theme: theme)
        XCTAssertEqual(runs.count, 1, "three quoted rows read as one unbroken bar")
        let rows = layout.rows
        XCTAssertEqual(runs[0].minY, rows[0].top, accuracy: 0.5)
        XCTAssertEqual(runs[0].maxY, rows[2].top + rows[2].height, accuracy: 0.5)
        XCTAssertEqual(runs[0].width, theme.quoteBarWidth)
    }

    func testAnEmptyQuotedLineStillCarriesItsBar() {
        // The row shape core emits for a bare `> ` — a gutter and nothing else,
        // which is an empty line inside a quote. It has no text to hang a bar
        // beside, but the bar is the block's, not the text's: a writer who opens
        // a quote and hasn't typed into it yet still has to see the quote.
        let dv = docView([
            row([gutter(1), mkRun("first")]),
            row([gutter(1)]),
            row([gutter(1), mkRun("third")]),
        ])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        XCTAssertEqual(layout.rows[1].quoteBars(theme: theme).count, 1,
                       "the empty line is quoted too")
        XCTAssertEqual(BlockChrome.quoteBarRuns(layout.rows, theme: theme).count, 1,
                       "and the bar runs unbroken through it, not in two pieces")
    }

    func testProseBetweenQuotesBreaksTheBarInTwo() {
        let dv = docView([
            row([gutter(1), mkRun("first quote")]),
            row([mkRun("unquoted prose")]),
            row([gutter(1), mkRun("second quote")]),
        ])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        XCTAssertEqual(BlockChrome.quoteBarRuns(layout.rows, theme: theme).count, 2)
    }

    func testNestedQuoteRunsAreTrackedPerLevel() {
        // Outer bar spans all three rows; the inner one only the middle row.
        let dv = docView([
            row([gutter(1), mkRun("outer")]),
            row([gutter(2), mkRun("inner")]),
            row([gutter(1), mkRun("outer again")]),
        ])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 400)
        let runs = BlockChrome.quoteBarRuns(layout.rows, theme: theme).sorted { $0.height > $1.height }
        XCTAssertEqual(runs.count, 2)
        let rows = layout.rows
        XCTAssertEqual(runs[0].minY, rows[0].top, accuracy: 0.5, "the outer bar covers every row")
        XCTAssertEqual(runs[0].maxY, rows[2].top + rows[2].height, accuracy: 0.5)
        XCTAssertEqual(runs[1].minY, rows[1].top, accuracy: 0.5, "the inner bar covers only the nested row")
        XCTAssertEqual(runs[1].maxY, rows[1].top + rows[1].height, accuracy: 0.5)
        XCTAssertGreaterThan(runs[1].minX, runs[0].minX)
    }

    func testUnquotedFrameDrawsNoBars() {
        let layout = EditorLayout(docView([row([mkRun("plain")])]), theme: theme, wrapWidth: 400)
        XCTAssertTrue(BlockChrome.quoteBarRuns(layout.rows, theme: theme).isEmpty)
    }

    // MARK: hanging indent

    func testWrappedQuoteHangsUnderItsOwnText() throws {
        let long = "the quick brown fox jumps over the lazy dog and then keeps on running"
        let dv = docView([row([gutter(1), mkRun(long)])])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 160)
        let rl = layout.rows[0]
        try XCTAssertGreaterThan(rl.wrapped.count, 1)
        XCTAssertEqual(rl.wrapped[0].indent, 0, "the first line is inset by the gutter's own glyphs")
        for wl in rl.wrapped.dropFirst() {
            XCTAssertEqual(wl.indent, rl.shaped.prefixWidth, accuracy: 0.5,
                           "continuations hang clear of the bar")
        }
        // A caret on the second visual line draws at the hanging indent, not the margin.
        let secondLine = try XCTUnwrap(layout.rect(row: 0, ch: rl.wrapped[0].length))
        XCTAssertEqual(secondLine.minX, theme.padding.left + rl.shaped.prefixWidth, accuracy: 1.0)
    }

    func testHitOnAHangingLineAccountsForTheIndent() {
        // A click at the left edge of a continuation line lands at that line's
        // first character, not somewhere shifted by the indent.
        let long = "the quick brown fox jumps over the lazy dog and then keeps on running"
        let dv = docView([row([gutter(1), mkRun(long)])])
        let layout = EditorLayout(dv, theme: theme, wrapWidth: 160)
        let rl = layout.rows[0]
        let secondLineStart = rl.wrapped[1].start
        let p = CGPoint(x: theme.padding.left + rl.shaped.prefixWidth + 0.5,
                        y: rl.top + rl.lineHeight * 1.5)
        let (r, ch) = layout.hit(p)
        XCTAssertEqual(r, 0)
        XCTAssertEqual(ch, secondLineStart, "the hit resolves to the start of the hanging line")
    }
}
