//  ToolbarPagingTests.swift
//
//  The `.bar` style's paging arithmetic — which groups fit a width, and where a
//  remembered page lands when the count changes. Driven with round numbers
//  rather than the bar's own metrics, so a failure reads as arithmetic rather
//  than as a point size.

import XCTest
@testable import LeafUI

final class ToolbarPagingTests: XCTestCase {
    /// Padding 10 a side, a separator 10 wide, a pager 50 wide.
    private let paging = ToolbarPaging(edgePadding: 10, separatorWidth: 10, pagerWidth: 50)
    /// Four groups: 100 + 10 + 100 + 10 + 40 + 10 + 40 = 310 across, 330 with
    /// the padding.
    private let widths: [CGFloat] = [100, 100, 40, 40]

    func testEverythingOnOnePageWhenItFits() {
        XCTAssertEqual(paging.pages(of: widths, in: 330), [[0, 1, 2, 3]])
        XCTAssertEqual(paging.pages(of: widths, in: 1000), [[0, 1, 2, 3]])
    }

    func testUnmeasuredIsOnePage() {
        // Before the first layout there is no width; the chevrons must not
        // blink in on the first frame and out on the second.
        XCTAssertEqual(paging.pages(of: widths, in: 0), [[0, 1, 2, 3]])
        XCTAssertEqual(paging.pages(of: widths, in: -1), [[0, 1, 2, 3]])
    }

    func testNoGroupsIsNoPages() {
        XCTAssertEqual(paging.pages(of: [], in: 300), [])
    }

    func testAPointShortSplitsAtAGroupBoundary() {
        // 329 across: the row is a point too wide, so it pages. The pager and
        // its separator come off the top — 329 - 20 - 10 - 50 = 249 per page —
        // and the second 100-wide group (100 + 10 + 100 = 210) fits beside the
        // first, but the 40 after it (210 + 10 + 40 = 260) does not.
        XCTAssertEqual(paging.pages(of: widths, in: 329), [[0, 1], [2, 3]])
    }

    func testAGroupIsNeverSplit() {
        // 240 across: 240 - 20 - 10 - 50 = 160 per page. The first 100 fits
        // and the next 100 (100 + 10 + 100 = 210) does not, so the page ends
        // there — groups are taken in order and whole, never partly. Then
        // 100 + 10 + 40 = 150 fits the second page and the last 40 does not.
        XCTAssertEqual(paging.pages(of: widths, in: 240), [[0], [1, 2], [3]])
    }

    func testAGroupWiderThanAPageStillGetsOne() {
        // Narrower than any group: each gets a page of its own, clipped,
        // rather than the packing giving up.
        XCTAssertEqual(paging.pages(of: widths, in: 100), [[0], [1], [2], [3]])
    }

    func testPagingBeginsThePointTheRowStopsFitting() {
        // Two 100-wide groups: 210 across, 230 with the padding. At 230 the
        // row fits and there is no pager; at 229 it pages, and a page holds
        // 229 - 20 - 10 - 50 = 149, so one group each.
        let two: [CGFloat] = [100, 100]
        XCTAssertEqual(paging.pages(of: two, in: 230), [[0, 1]])
        XCTAssertEqual(paging.pages(of: two, in: 229), [[0], [1]])
    }

    func testClampKeepsAPageThatStillExists() {
        XCTAssertEqual(ToolbarPaging.clamp(1, to: 3), 1)
        XCTAssertEqual(ToolbarPaging.clamp(2, to: 3), 2)
    }

    func testClampLandsOnTheLastPageWhenTheCountShrinks() {
        // On page 3 of 4, the window widens and there are 2: the last page
        // shows, not the first. Widen further to one page and there is no
        // pager at all — page 0.
        XCTAssertEqual(ToolbarPaging.clamp(3, to: 2), 1)
        XCTAssertEqual(ToolbarPaging.clamp(3, to: 1), 0)
        XCTAssertEqual(ToolbarPaging.clamp(3, to: 0), 0)
    }

    func testClampNeverGoesNegative() {
        XCTAssertEqual(ToolbarPaging.clamp(-1, to: 3), 0)
    }

    func testARememberedPageComesBackWhenTheWidthDoes() {
        // The view keeps the page it was turned to and clamps at draw time,
        // so a narrow → wide → narrow window shows the same page it left. The
        // clamp is pure, so this is only that it does not lose information.
        let remembered = 2
        XCTAssertEqual(ToolbarPaging.clamp(remembered, to: 1), 0)
        XCTAssertEqual(ToolbarPaging.clamp(remembered, to: 3), 2)
    }
}
