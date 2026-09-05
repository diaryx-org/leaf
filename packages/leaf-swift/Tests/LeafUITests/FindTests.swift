//  FindTests.swift
//
//  The macOS view as the system find bar's client: the string it searches, the
//  UTF-16 ranges it hands back, and the boxes a match is highlighted with. Over a
//  real `LeafDoc`, since the whole point is the byte⇄UTF-16 crossing.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class FindTests: XCTestCase {
    private func laidOut(_ source: String) throws -> LeafTextView {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.layout()
        return view
    }

    func testTheSearchedStringIsTheVisibleText() throws {
        let view = try laidOut("# leaf\n\nA **bold** leaf.\n")
        XCTAssertEqual(view.string, "leaf\nA bold leaf.")
    }

    func testAMatchHasABoxOnItsLineAndSelectsExactly() throws {
        let view = try laidOut("# leaf\n\nA **bold** leaf.\n")
        let text = view.string as NSString
        let second = text.range(of: "leaf", options: .backwards)
        XCTAssertNotEqual(second.location, NSNotFound)

        let boxes = try XCTUnwrap(view.rects(forCharacterRange: second))
        XCTAssertEqual(boxes.count, 1, "one line, one box")
        let box = boxes[0].rectValue
        XCTAssertGreaterThan(box.width, 10)
        XCTAssertGreaterThan(box.height, 10)
        XCTAssertTrue(view.bounds.contains(box), "the box lies in the view: \(box) in \(view.bounds)")

        view.selectedRanges = [NSValue(range: second)]
        XCTAssertEqual(view.firstSelectedRange, second, "the selection round-trips in the finder's units")
        XCTAssertEqual(view.attributedSubstring(forProposedRange: second, actualRange: nil)?.string, "leaf")
    }

    func testTheWholeStringIsDrawnInThisOneView() throws {
        // The finder asks which view a range is drawn in before it will dim or
        // highlight anything; the answer is always this view over the whole text.
        let view = try laidOut("# leaf\n\nA **bold** leaf.\n")
        var effective = NSRange()
        let host = view.contentView(at: 3, effectiveCharacterRange: &effective)
        XCTAssertTrue(host === view)
        XCTAssertEqual(effective, NSRange(location: 0, length: (view.string as NSString).length))
        XCTAssertTrue(view.responds(to: NSSelectorFromString("contentViewAtIndex:effectiveCharacterRange:")),
                      "the optional requirement must be satisfied under its Objective-C name")
    }

    func testTheVisibleRangeCoversAShortDocument() throws {
        let view = try laidOut("# leaf\n\nA **bold** leaf.\n")
        let visible = view.visibleCharacterRanges.map(\.rangeValue)
        XCTAssertEqual(visible.count, 1)
        XCTAssertEqual(visible[0], NSRange(location: 0, length: (view.string as NSString).length),
                       "everything is on screen, so everything is visible")
    }
}
#endif
