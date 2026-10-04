//  AccessibilityTests.swift
//
//  The macOS view as VoiceOver reads it: visual lines that partition the
//  text and agree with each other, the caret's line, composed characters,
//  ranges on screen, and the look of the text in accessibility attributes.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class AccessibilityTests: XCTestCase {
    private func editor(_ source: String, width: CGFloat = 600) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: width, height: 400)
        view.layout()
        return view
    }

    private func length(_ view: LeafTextView) -> Int { view.accessibilityNumberOfCharacters() }

    /// Every line, in order, as its range.
    private func lines(_ view: LeafTextView) -> [NSRange] {
        view.accessibilityLines.indices.map { view.accessibilityRange(forLine: $0) }
    }

    private let wrapping = Array(repeating: "lorem ipsum dolor", count: 12).joined(separator: " ")

    func testLinesPartitionTheTextAndAgreeWithEachOther() throws {
        let view = try editor("# Title\n\n" + wrapping + "\n\n- one\n- two\n", width: 320)
        let ranges = lines(view)
        XCTAssertGreaterThan(ranges.count, 5, "a heading, a wrapped paragraph, two items")
        XCTAssertEqual(ranges.first?.location, 0)
        XCTAssertEqual(ranges.last.map(NSMaxRange), length(view))
        for (a, b) in zip(ranges, ranges.dropFirst()) {
            XCTAssertEqual(NSMaxRange(a), b.location, "no gap and no overlap")
            XCTAssertGreaterThan(a.length, 0)
        }
        // Every index is on the line whose range holds it.
        for index in 0..<length(view) {
            let line = view.accessibilityLine(for: index)
            XCTAssertTrue(NSLocationInRange(index, view.accessibilityRange(forLine: line)), "index \(index)")
        }
    }

    func testAWrappedParagraphIsSeveralLines() throws {
        let view = try editor(wrapping + "\n", width: 320)
        let wrapped = view.layoutEngine.rows[0].wrapped.count
        XCTAssertGreaterThan(wrapped, 1)
        XCTAssertEqual(lines(view).count, wrapped)
        // The second line reads as the second wrapped line draws.
        let second = try XCTUnwrap(view.accessibilityString(for: view.accessibilityRange(forLine: 1)))
        XCTAssertEqual(second, view.layoutEngine.rows[0].wrapped[1].attributed.string)
    }

    func testTheInsertionPointsLineFollowsTheCaretDown() throws {
        let view = try editor(wrapping + "\n\nlast\n", width: 320)
        XCTAssertEqual(view.accessibilityInsertionPointLineNumber(), 0)
        view.doCommand(by: #selector(NSResponder.moveDown(_:)))
        XCTAssertEqual(view.accessibilityInsertionPointLineNumber(), 1, "the visual line, not the paragraph")
        view.doCommand(by: #selector(NSResponder.moveToEndOfDocument(_:)))
        let last = view.accessibilityInsertionPointLineNumber()
        XCTAssertEqual(last, lines(view).count - 1)
        XCTAssertEqual(view.accessibilityLine(for: view.accessibilitySelectedTextRange().location), last)
    }

    func testOutOfRangeLinesAndIndicesAreNotFound() throws {
        let view = try editor("short\n")
        XCTAssertEqual(view.accessibilityRange(forLine: 99).location, NSNotFound)
        XCTAssertEqual(view.accessibilityRange(for: 99).location, NSNotFound)
    }

    func testACharacterIsComposed() throws {
        // Outside the BMP: one character, two UTF-16 units. (A modified emoji
        // is spelled by its base alone in the visible text — one character
        // per caret stop is core's contract — so it would test nothing more.)
        let view = try editor("a😀b\n")
        XCTAssertEqual(view.accessibilityRange(for: 1), NSRange(location: 1, length: 2))
        XCTAssertEqual(view.accessibilityRange(for: 2), NSRange(location: 1, length: 2))
        XCTAssertEqual(view.accessibilityRange(for: 3), NSRange(location: 3, length: 1))
    }

    func testARangesFrameIsWhereItIsDrawn() throws {
        let view = try editor("hello world\n")
        let world = NSRange(location: 6, length: 5)
        // Not in a window: answered in the view's own coordinates.
        let frame = view.accessibilityFrame(for: world)
        let box = try XCTUnwrap(view.layoutEngine.rangeRects(fromByte: 6, toByte: 11, in: view.doc).first)
        XCTAssertEqual(frame.minX, box.minX, accuracy: 0.5)
        XCTAssertEqual(frame.width, box.width, accuracy: 0.5)
    }

    func testTheAttributedStringCarriesTheLook() throws {
        let view = try editor("plain **bold** ~~gone~~\n")
        let text = try XCTUnwrap(view.accessibilityAttributedString(for: NSRange(location: 0, length: 15)))
        XCTAssertEqual(text.string, "plain bold gone")
        let bold = try XCTUnwrap(text.attribute(.accessibilityFont, at: 7, effectiveRange: nil) as? [String: Any])
        let plain = try XCTUnwrap(text.attribute(.accessibilityFont, at: 1, effectiveRange: nil) as? [String: Any])
        XCTAssertNotEqual(bold["AXFontName"] as? String, plain["AXFontName"] as? String)
        XCTAssertEqual(text.attribute(.accessibilityStrikethrough, at: 12, effectiveRange: nil) as? Bool, true)
    }

    func testAStyleRunEndsWhereTheFontChanges() throws {
        let view = try editor("plain **bold** tail\n")
        XCTAssertEqual(view.accessibilityStyleRange(for: 7), NSRange(location: 6, length: 4))
    }

    func testTheLineTableFollowsAnEdit() throws {
        let view = try editor("one\n")
        XCTAssertEqual(lines(view).count, 1)
        view.command { $0.moveDocEnd(extend: false) }
        view.insertText("\n\ntwo", replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertEqual(lines(view).count, 2)
        XCTAssertEqual(view.accessibilityString(for: view.accessibilityRange(forLine: 1)), "two")
    }
}
#endif
