//  FindTests.swift
//
//  The macOS view as the system find bar's client, and the iOS view as the find
//  panel's: the string each searches, the UTF-16 ranges handed across, the boxes
//  a match is lit with, and replacing one. Over a real `LeafDoc`, since the
//  whole point is the byte⇄UTF-16 crossing.

import XCTest
import LeafFFI
@testable import LeafUI

/// The source bytes `from..<to` — what a range covers in the file, markup and
/// all, where `textInRange` would answer with the visible text.
private func sourceBytes(_ doc: LeafDoc, _ from: Int, _ to: Int) -> String {
    String(decoding: Array(doc.source().utf8)[from..<to], as: UTF8.self)
}

/// The search itself, which the iOS view does and the Mac's find bar does for
/// its client.
final class TextSearchTests: XCTestCase {
    func testEveryOccurrenceFirstToLastWithoutOverlap() {
        XCTAssertEqual(TextSearch.matches(of: "aa", in: "aaaa b aa"),
                       [NSRange(location: 0, length: 2), NSRange(location: 2, length: 2), NSRange(location: 7, length: 2)])
    }

    func testCaseIsTheOptionsBusiness() {
        XCTAssertEqual(TextSearch.matches(of: "leaf", in: "Leaf leaf", options: [.caseInsensitive]).count, 2)
        XCTAssertEqual(TextSearch.matches(of: "leaf", in: "Leaf leaf", options: []), [NSRange(location: 5, length: 4)])
    }

    func testWordMatching() {
        let text = "leaf leaflet sunleaf leaf_x leaf"
        XCTAssertEqual(TextSearch.matches(of: "leaf", in: text, word: .contains).count, 5)
        XCTAssertEqual(TextSearch.matches(of: "leaf", in: text, word: .startsWith).map(\.location), [0, 5, 21, 28])
        XCTAssertEqual(TextSearch.matches(of: "leaf", in: text, word: .fullWord).map(\.location), [0, 28])
    }

    func testRangesAreUTF16() {
        // 🌿 is two UTF-16 units; the match after it starts at 3, not 2.
        XCTAssertEqual(TextSearch.matches(of: "leaf", in: "🌿 leaf"), [NSRange(location: 3, length: 4)])
        XCTAssertEqual(TextSearch.matches(of: "leaf", in: "🌿leaf", word: .startsWith), [NSRange(location: 2, length: 4)],
                       "a picture is not a letter, so the word starts after it")
    }

    func testAnEmptyQueryFindsNothing() {
        XCTAssertEqual(TextSearch.matches(of: "", in: "anything"), [])
    }

    func testTheCrossingIsCoresMapping() throws {
        // `**` is hidden in the visible text, so the second "leaf" is at a
        // different UTF-16 index than byte offset, and the crossing lands on it.
        let doc = try LeafDoc(source: "# leaf\n\nA **bold** 🌿 leaf.\n", format: "markdown")
        let text = doc.visibleText()
        let second = (text as NSString).range(of: "leaf", options: .backwards)
        let (from, to) = doc.matchBounds(second)
        XCTAssertEqual(sourceBytes(doc, from, to), "leaf")
        XCTAssertEqual(doc.utf16Range(fromByte: from, toByte: to), second, "and back again")
    }

    func testAMatchEndsBeforeTheMarkupThatClosesAroundIt() throws {
        let doc = try LeafDoc(source: "A **bold** and *🌿* and [link](https://x.dev) and `code`.\n", format: "markdown")
        let text = doc.visibleText() as NSString
        for word in ["bold", "🌿", "link", "code"] {
            let (from, to) = doc.matchBounds(text.range(of: word))
            XCTAssertEqual(sourceBytes(doc, from, to), word, "the word's own bytes, no delimiter")
        }
        // Where `byteBounds` — a caret stop at each end — goes past the `**`.
        let (_, stop) = doc.byteBounds(text.range(of: "bold"))
        XCTAssertEqual(stop, 10)
        XCTAssertEqual(doc.matchBounds(text.range(of: "bold")).to, 8)
    }

    func testAMatchAcrossABlockGapEndsAtTheNextBlock() throws {
        let doc = try LeafDoc(source: "one\n\ntwo\n", format: "markdown")
        let (from, to) = doc.matchBounds((doc.visibleText() as NSString).range(of: "one\n"))
        XCTAssertEqual(from, 0)
        XCTAssertEqual(to, 5, "the gap's stop, not the middle of it")
    }
}

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit

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

    func testReplacingAMatchKeepsTheMarkupAroundIt() throws {
        let view = try laidOut("# leaf\n\nA **bold** leaf.\n")
        let bold = (view.string as NSString).range(of: "bold")
        view.selectedRanges = [NSValue(range: bold)]
        XCTAssertEqual(view.firstSelectedRange, bold)
        view.replaceCharacters(in: bold, with: "brave")
        XCTAssertEqual(view.sourceText(), "# leaf\n\nA **brave** leaf.\n")
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

#if canImport(UIKit)
import UIKit

/// The iOS view as `UITextSearching`: what the find panel's search returns,
/// how a match is lit and selected, and what replacing does to the source.
final class FindTests: XCTestCase {
    private func laidOut(_ source: String) throws -> (LeafDoc, LeafTextView) {
        let doc = try LeafDoc(source: source, format: "markdown")
        let view = LeafTextView(doc: doc, theme: .default)
        view.frame = CGRect(x: 0, y: 0, width: 402, height: 700)
        view.layoutIfNeeded()
        return (doc, view)
    }

    private func search(_ view: LeafTextView, _ query: String) -> [LeafTextRange] {
        view.foundRanges(of: query, options: UITextSearchOptions())
    }

    private func source(_ doc: LeafDoc, _ range: LeafTextRange) -> String {
        sourceBytes(doc, range.from.offset, range.to.offset)
    }

    func testTheSearchIsOverTheVisibleText() throws {
        let (doc, view) = try laidOut("# leaf\n\nA **bold** leaf.\n")
        let found = search(view, "leaf")
        XCTAssertEqual(found.count, 2)
        XCTAssertEqual(found.map { source(doc, $0) }, ["leaf", "leaf"], "each is the word's own bytes")
        XCTAssertEqual(search(view, "**").count, 0, "hidden markup is not text to find in the rendered view")
        XCTAssertEqual(search(view, "bold leaf").count, 1, "and a match runs across where it was")
    }

    func testTheSourceViewSearchesTheSource() throws {
        let (doc, view) = try laidOut("# leaf\n\nA **bold** leaf.\n")
        view.command { $0.toggleView() }
        let found = search(view, "**")
        XCTAssertEqual(found.count, 2, "what is visible there is the markup")
        XCTAssertEqual(found.map { source(doc, $0) }, ["**", "**"])
    }

    func testTheFoundRangesAreInDocumentOrder() throws {
        let (_, view) = try laidOut("leaf and leaf\n")
        let found = search(view, "leaf")
        XCTAssertEqual(view.compare(found[0], toRange: found[1], document: nil), .orderedAscending)
        XCTAssertEqual(view.compare(found[1], toRange: found[0], document: nil), .orderedDescending)
        XCTAssertEqual(view.compare(found[0], toRange: found[0], document: nil), .orderedSame)
    }

    func testADecoratedMatchIsLitOverItsOwnBoxes() throws {
        let (doc, view) = try laidOut("# leaf\n\nA **bold** leaf.\n")
        let second = try XCTUnwrap(search(view, "leaf").last)
        XCTAssertTrue(view.foundTextRects.isEmpty)

        view.decorate(foundTextRange: second, document: nil, usingStyle: .found)
        let lit = view.foundTextRects
        XCTAssertEqual(lit.count, 1, "one line, one box")
        XCTAssertEqual(lit[0].style, .found)
        XCTAssertEqual(lit[0].rect, view.layoutEngine.rangeRects(fromByte: second.from.offset, toByte: second.to.offset, in: doc)[0])
        XCTAssertGreaterThan(lit[0].rect.width, 10)
        XCTAssertTrue(view.bounds.contains(lit[0].rect))

        view.decorate(foundTextRange: second, document: nil, usingStyle: .highlighted)
        XCTAssertEqual(view.foundTextRects.map(\.style), [.highlighted], "restyled, not doubled")
        view.decorate(foundTextRange: second, document: nil, usingStyle: .normal)
        XCTAssertTrue(view.foundTextRects.isEmpty, "normal is no decoration")

        for range in search(view, "leaf") { view.decorate(foundTextRange: range, document: nil, usingStyle: .found) }
        XCTAssertEqual(view.foundTextRects.count, 2)
        view.clearAllDecoratedFoundText()
        XCTAssertTrue(view.foundTextRects.isEmpty)
    }

    func testTheHighlightedMatchIsTheSelection() throws {
        let (doc, view) = try laidOut("A **bold** word.\n")
        let bold = try XCTUnwrap(search(view, "bold").first)
        view.willHighlight(foundTextRange: bold, document: nil)
        let selected = try XCTUnwrap(view.selectedTextRange as? LeafTextRange)
        XCTAssertEqual(selected.from.offset, bold.from.offset)
        XCTAssertEqual(selected.to.offset, bold.to.offset, "the word exactly, not one stop short of it")
        XCTAssertEqual(source(doc, selected), "bold")
    }

    func testReplacingAMatchKeepsTheMarkupAroundIt() throws {
        let (doc, view) = try laidOut("# leaf\n\nA **bold** leaf.\n")
        XCTAssertTrue(view.supportsTextReplacement)
        let bold = try XCTUnwrap(search(view, "bold").first)
        view.replace(foundTextRange: bold, document: nil, withText: "brave")
        XCTAssertEqual(doc.source(), "# leaf\n\nA **brave** leaf.\n")
    }

    func testReplaceAllReplacesEveryMatchTheOptionsAllow() throws {
        let (doc, view) = try laidOut("# Leaf\n\nA **leaf**, a leaflet, a leaf.\n")
        view.replaceAll(queryString: "leaf", options: UITextSearchOptions(), withText: "tree")
        XCTAssertEqual(doc.source(), "# Leaf\n\nA **tree**, a treelet, a tree.\n", "case-sensitive, as the options say")
    }

    func testAReaderIsNotOfferedReplacement() throws {
        let (_, view) = try laidOut("leaf\n")
        view.isReadOnly = true
        XCTAssertFalse(view.supportsTextReplacement)
        XCTAssertFalse(view.shouldReplace(foundTextRange: search(view, "leaf")[0], document: nil, withText: "x"))
    }
}
#endif
