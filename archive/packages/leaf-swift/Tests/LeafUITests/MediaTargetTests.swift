//  MediaTargetTests.swift
//
//  Which attachment a menu offers, and when it offers none. The two inputs come
//  from places a unit test has no view for — a hit-tested media box and a host's
//  closure — so `showableMediaSource` takes both as values, and this is the test
//  that shape exists for. The caret half (`imageDestinationAtCaret`) is core's
//  and tested there; what is checked here is that it reaches Swift at all.

import XCTest
import LeafFFI
@testable import LeafUI

final class MediaTargetTests: XCTestCase {
    private func doc(_ source: String, caret offset: UInt32) throws -> LeafDoc {
        let d = try LeafDoc(source: source, format: "markdown")
        _ = d.setSelectionOffsets(anchor: offset, focus: offset)
        return d
    }

    func testCaretInsideAnImageAnswersWithItsSource() throws {
        let d = try doc("![a](cat.png) after\n", caret: 3)
        XCTAssertEqual(d.mediaSourceAtCaret(), "cat.png")
    }

    func testCaretInProseAnswersWithNothing() throws {
        let d = try doc("![a](cat.png) after\n", caret: 16)
        XCTAssertNil(d.mediaSourceAtCaret())
    }

    /// The box wins: a click inside a block image's box is unambiguous about
    /// which attachment was meant, where the caret it placed may have landed at
    /// the block's edge.
    func testTheBoxUnderThePointerOutranksTheCaret() {
        XCTAssertEqual(
            showableMediaSource(box: "clip.mp4", caret: "cat.png", canShow: true),
            "clip.mp4")
    }

    /// A video or audio box has no image node under it, so the caret answers
    /// nothing and the hit-test is the whole answer.
    func testTheBoxAloneIsEnough() {
        XCTAssertEqual(showableMediaSource(box: "clip.mp4", caret: nil, canShow: true), "clip.mp4")
    }

    func testTheCaretAnswersWhenThePointerHitNoBox() {
        XCTAssertEqual(showableMediaSource(box: nil, caret: "cat.png", canShow: true), "cat.png")
    }

    /// No host hook, no entry — the `onEditLink` rule.
    func testNoHookMeansNoEntry() {
        XCTAssertNil(showableMediaSource(box: "clip.mp4", caret: "cat.png", canShow: false))
    }

    func testNeitherSourceMeansNoEntry() {
        XCTAssertNil(showableMediaSource(box: nil, caret: nil, canShow: true))
    }

    /// A `<video>` that named no source at all: core reports an empty `src`, and
    /// there is nothing there for a host to show.
    func testAnEmptySourceIsNoAttachment() {
        XCTAssertNil(showableMediaSource(box: "", caret: nil, canShow: true))
        XCTAssertEqual(showableMediaSource(box: "", caret: "cat.png", canShow: true), "cat.png")
    }
}
