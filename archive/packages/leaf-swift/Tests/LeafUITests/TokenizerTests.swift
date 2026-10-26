//  TokenizerTests.swift
//
//  The iOS view's line tokenizer, as UIKit walks it: every step from the end
//  of the last enclosing range. A range that ends where it was asked about
//  holds that walk in place — iOS 26's autocorrect underline spun the main
//  thread on the space after a word at the end of a page.

#if canImport(UIKit)
import XCTest
import UIKit
import LeafFFI
@testable import LeafUI

final class TokenizerTests: XCTestCase {
    private func laidOut(_ source: String) throws -> (LeafDoc, LeafTextView) {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = CGRect(x: 0, y: 0, width: 240, height: 700)
        view.layoutIfNeeded()
        return (doc, view)
    }

    private func enclosing(_ view: LeafTextView, _ o: Int, _ direction: UITextStorageDirection) -> LeafTextRange? {
        view.tokenizer.rangeEnclosingPosition(LeafTextPosition(o), with: .line,
                                              inDirection: UITextDirection(rawValue: direction.rawValue))
            as? LeafTextRange
    }

    /// Every offset in the document, caret stop by caret stop.
    private func offsets(_ doc: LeafDoc) -> [Int] {
        var all = [0]
        while true {
            let next = Int(doc.stepOffset(off: UInt32(all.last!), delta: 1))
            guard next > all.last! else { return all }
            all.append(next)
        }
    }

    func testTheEndOfTheDocumentEnclosesNothingReadingOn() throws {
        let (doc, view) = try laidOut("Hello")
        XCTAssertNil(enclosing(view, Int(doc.docEndOffset()), .forward))
        XCTAssertNil(enclosing(view, 0, .backward))
    }

    func testEveryRangeHasTheWordAndMovesTheWalkOn() throws {
        // A soft-wrapped paragraph, an empty line, and a hard break between.
        let (doc, view) = try laidOut("Hello\n\nthe quick brown fox jumps over the lazy dog and keeps running\n")
        for o in offsets(doc) {
            if let r = enclosing(view, o, .forward) {
                XCTAssertTrue(r.from.offset <= o && o < r.to.offset, "forward from \(o): \(r.from.offset)..<\(r.to.offset)")
            }
            if let r = enclosing(view, o, .backward) {
                XCTAssertTrue(r.from.offset < o && o <= r.to.offset, "backward from \(o): \(r.from.offset)...\(r.to.offset)")
            }
        }
    }

    func testAWalkOverAWordEnds() throws {
        let (doc, view) = try laidOut("Hello")
        let end = Int(doc.docEndOffset())
        let line = try XCTUnwrap(enclosing(view, 0, .forward))
        XCTAssertEqual(line.to.offset, end)
        XCTAssertNil(enclosing(view, line.to.offset, .forward), "the next step is off the page, not the same line again")
    }
}
#endif
