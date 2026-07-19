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
        let l1 = EditorLayout(docView([row([mkRun("alpha")]), row([mkRun("beta")])]), theme: theme, cache: &cache)
        // Edit row 0 only; row 1 is byte-identical.
        let l2 = EditorLayout(docView([row([mkRun("alphaX")]), row([mkRun("beta")])]), theme: theme, cache: &cache)
        XCTAssertTrue(l1.rows[1].attributed === l2.rows[1].attributed, "unchanged row reuses its shaped text")
        XCTAssertFalse(l1.rows[0].attributed === l2.rows[0].attributed, "changed row is re-shaped")
    }

    func testCacheReuseSurvivesRowInsertion() {
        var cache: [Row: ShapedRow] = [:]
        let a = row([mkRun("a")])
        let b = row([mkRun("b")])
        let l1 = EditorLayout(docView([a, b]), theme: theme, cache: &cache)
        // Insert a new first row: a and b shift down one but are unchanged.
        let l2 = EditorLayout(docView([row([mkRun("new")]), a, b]), theme: theme, cache: &cache)
        XCTAssertTrue(l1.rows[0].attributed === l2.rows[1].attributed, "row reused despite shifting position")
        XCTAssertTrue(l1.rows[1].attributed === l2.rows[2].attributed)
    }

    func testCacheEvictsRowsNoLongerPresent() {
        var cache: [Row: ShapedRow] = [:]
        _ = EditorLayout(docView([row([mkRun("keep")]), row([mkRun("drop")])]), theme: theme, cache: &cache)
        _ = EditorLayout(docView([row([mkRun("keep")])]), theme: theme, cache: &cache)
        XCTAssertEqual(cache.count, 1, "the removed row is evicted; the cache stays bounded to the document")
        XCTAssertNotNil(cache[row([mkRun("keep")])])
    }
}
