//  LandingTests.swift
//
//  The arithmetic behind arriving somewhere: where a landing scrolls to, and how
//  long its flash lasts. Both are pure functions precisely so they can be pinned
//  here — the bug they exist to prevent (a verse that lands in a different place
//  every time you follow the same link) is invisible in a screenshot of any one
//  arrival.

import XCTest
@testable import LeafUI

final class LandingTests: XCTestCase {
    /// A document taller than the viewport, so there is room to scroll in both
    /// directions.
    private let visible: CGFloat = 600
    private let document: CGFloat = 5000

    private func block(at y: CGFloat, height: CGFloat = 20) -> CGRect {
        CGRect(x: 0, y: y, width: 400, height: height)
    }

    func testTheSameBlockLandsInTheSamePlaceFromAboveOrBelow() {
        // The whole point. The minimum-scroll rule this replaces put the block at
        // the bottom edge when arriving from above and the top edge from below;
        // where the reader had been is not supposed to change where they arrive.
        let target = block(at: 2000)
        let landing = Landing.scrollTop(for: target, visibleHeight: visible,
                                        documentHeight: document)
        XCTAssertEqual(landing, 2000 - Landing.lead)
        // And it does not depend on the current scroll position at all — there is
        // nowhere in the signature to put one.
    }

    func testTheBlockSitsALeadInBelowTheTop() {
        let landing = Landing.scrollTop(for: block(at: 1000), visibleHeight: visible,
                                        documentHeight: document)
        XCTAssertEqual(1000 - landing, Landing.lead,
                       "the block's distance from the top of the viewport")
    }

    func testLandingNearTheStartDoesNotScrollAboveTheDocument() {
        // The first block of a document has nothing above it to show, so the lead
        // gives way rather than scrolling to a negative offset.
        XCTAssertEqual(Landing.scrollTop(for: block(at: 10), visibleHeight: visible,
                                         documentHeight: document), 0)
    }

    func testLandingNearTheEndDoesNotScrollPastIt() {
        let landing = Landing.scrollTop(for: block(at: 4980), visibleHeight: visible,
                                        documentHeight: document)
        XCTAssertEqual(landing, document - visible, "flush with the document's end")
    }

    func testADocumentShorterThanTheViewportNeverScrolls() {
        XCTAssertEqual(Landing.scrollTop(for: block(at: 100), visibleHeight: visible,
                                         documentHeight: 300), 0)
    }

    func testABlockTallerThanTheViewportIsReadFromItsTop() {
        // Honouring the lead here would push the block's own first line off the
        // top of the screen to make room for what comes before it.
        let landing = Landing.scrollTop(for: block(at: 1000, height: 900),
                                        visibleHeight: visible, documentHeight: document)
        XCTAssertEqual(landing, 1000, "no lead-in when there is no room for one")
    }

    // ── the flash ─────────────────────────────────────────────────────────────

    func testTheFlashHoldsThenFadesThenStops() {
        XCTAssertEqual(Landing.opacity(elapsed: 0), 1)
        XCTAssertEqual(Landing.opacity(elapsed: Landing.hold), 1, "solid for the whole hold")
        let mid = try? XCTUnwrap(Landing.opacity(elapsed: Landing.hold + Landing.fade / 2))
        XCTAssertEqual(mid ?? 0, 0.5, accuracy: 0.01)
        // Nil rather than zero: it is what tells the view to stop redrawing, and
        // an opacity of zero would animate forever.
        XCTAssertNil(Landing.opacity(elapsed: Landing.hold + Landing.fade))
        XCTAssertNil(Landing.opacity(elapsed: 60))
    }
}
