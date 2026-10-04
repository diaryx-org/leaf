//  TextInputTests.swift
//
//  What the macOS view tells an input method through `NSTextInputClient`:
//  the text with its real attributes, a composition's clauses, and the
//  geometry a candidate window or the input-source indicator is placed by.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class TextInputTests: XCTestCase {
    private func editor(_ source: String, width: CGFloat = 600) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: width, height: 400)
        view.layout()
        return view
    }

    private let none = NSRange(location: NSNotFound, length: 0)

    func testAnInputMethodMayMarkClausesAndUnderlines() throws {
        let view = try editor("x\n")
        let keys = Set(view.validAttributesForMarkedText())
        XCTAssertEqual(keys, [.underlineStyle, .underlineColor, .markedClauseSegment])
    }

    func testTheSubstringCarriesTheFontItIsDrawnIn() throws {
        let view = try editor("plain **bold**\n")
        var actual = NSRange()
        let text = try XCTUnwrap(view.attributedSubstring(forProposedRange: NSRange(location: 0, length: 10),
                                                          actualRange: &actual))
        XCTAssertEqual(text.string, "plain bold")
        XCTAssertEqual(actual, NSRange(location: 0, length: 10))
        let bold = try XCTUnwrap(text.attribute(.font, at: 7, effectiveRange: nil) as? NSFont)
        let plain = try XCTUnwrap(text.attribute(.font, at: 1, effectiveRange: nil) as? NSFont)
        XCTAssertTrue(bold.fontDescriptor.symbolicTraits.contains(.bold))
        XCTAssertFalse(plain.fontDescriptor.symbolicTraits.contains(.bold))
    }

    func testAClausedCompositionIsDrawnAClauseAtATime() throws {
        let view = try editor("\n")
        // かん|じ — two clauses, the second being converted (thick).
        let marked = NSMutableAttributedString(string: "かんじ")
        marked.addAttributes([.markedClauseSegment: 0, .underlineStyle: NSUnderlineStyle.single.rawValue],
                             range: NSRange(location: 0, length: 2))
        marked.addAttributes([.markedClauseSegment: 1, .underlineStyle: NSUnderlineStyle.thick.rawValue],
                             range: NSRange(location: 2, length: 1))
        view.setMarkedText(marked, selectedRange: NSRange(location: 3, length: 0), replacementRange: none)
        XCTAssertTrue(view.hasMarkedText())
        let clauses = view.markedClauses
        XCTAssertEqual(clauses.count, 2)
        XCTAssertEqual(clauses.map(\.range), [NSRange(location: 0, length: 6), NSRange(location: 6, length: 3)],
                       "source bytes: each kana is three")
        XCTAssertEqual(clauses.map(\.selected), [false, true])
        view.insertText("漢字", replacementRange: none)
        XCTAssertFalse(view.hasMarkedText())
        XCTAssertTrue(view.markedClauses.isEmpty)
        XCTAssertEqual(view.sourceText(), "漢字\n")
    }

    func testWithoutThickUnderlinesTheSelectedClauseIsTheOneHoldingTheSelection() {
        let marked = NSMutableAttributedString(string: "abcd")
        marked.addAttribute(.markedClauseSegment, value: 0, range: NSRange(location: 0, length: 2))
        marked.addAttribute(.markedClauseSegment, value: 1, range: NSRange(location: 2, length: 2))
        let clauses = LeafTextView.clauses(of: marked, selectedRange: NSRange(location: 0, length: 2), at: 10)
        XCTAssertEqual(clauses.map(\.selected), [true, false])
        XCTAssertEqual(clauses.first?.range, NSRange(location: 10, length: 2))
    }

    func testAPatternedSingleUnderlineIsNotTheSelectedClause() {
        // Thin and dotted by word, then thick: the pattern bits make the first
        // style's raw value large, and it is still the lighter line.
        let marked = NSMutableAttributedString(string: "abcd")
        let dotted = NSUnderlineStyle([.single, .patternDot, .byWord]).rawValue
        marked.addAttribute(.underlineStyle, value: dotted, range: NSRange(location: 0, length: 2))
        marked.addAttribute(.underlineStyle, value: NSUnderlineStyle.thick.rawValue, range: NSRange(location: 2, length: 2))
        let clauses = LeafTextView.clauses(of: marked, selectedRange: NSRange(location: 4, length: 0), at: 0)
        XCTAssertEqual(clauses.map(\.selected), [false, true])
    }

    func testAPlainCompositionHasNoClauses() {
        let clauses = LeafTextView.clauses(of: NSAttributedString(string: "ni"),
                                           selectedRange: NSRange(location: 2, length: 0), at: 0)
        XCTAssertTrue(clauses.isEmpty)
    }

    func testTheFirstRectIsTheWholeFirstLineOfTheRange() throws {
        let words = Array(repeating: "lorem ipsum", count: 20).joined(separator: " ")
        let view = try editor(words + "\n", width: 300)
        let wrapped = view.layoutEngine.rows[0].wrapped
        XCTAssertGreaterThan(wrapped.count, 1)
        let all = NSRange(location: 2, length: 60)
        var actual = NSRange()
        let rect = view.firstRect(forCharacterRange: all, actualRange: &actual)
        // Not in a window, so in view coordinates.
        let firstLine = try XCTUnwrap(view.layoutEngine.rangeRects(fromByte: 2, toByte: 62, in: view.doc).first)
        XCTAssertEqual(rect.minX, firstLine.minX, accuracy: 0.5)
        XCTAssertEqual(rect.width, firstLine.width, accuracy: 0.5)
        XCTAssertGreaterThan(rect.width, 50, "a line, not a character")
        XCTAssertEqual(actual.location, 2)
        XCTAssertEqual(NSMaxRange(actual), wrapped[0].length, "cut back to the first line")
    }

    func testAnEmptyRangesRectIsTheCaretsBox() throws {
        let view = try editor("hello\n")
        var actual = NSRange()
        let rect = view.firstRect(forCharacterRange: NSRange(location: 2, length: 0), actualRange: &actual)
        XCTAssertEqual(actual, NSRange(location: 2, length: 0))
        XCTAssertGreaterThan(rect.height, 0)
    }

    func testTheVisibleSelectionAndDocumentRectsAreOnScreen() throws {
        let view = try editor("hello world\n")
        let window = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 600, height: 400),
                              styleMask: [.titled], backing: .buffered, defer: true)
        window.contentView = view
        view.layout()
        view.command { $0.selectRange(start: 6, end: 11) }
        let selection = view.unionRectInVisibleSelectedRange()
        let expected = window.convertToScreen(view.convert(
            try XCTUnwrap(view.layoutEngine.rangeRects(fromByte: 6, toByte: 11, in: view.doc).first), to: nil))
        XCTAssertEqual(selection.minX, expected.minX, accuracy: 0.5)
        XCTAssertEqual(selection.width, expected.width, accuracy: 0.5)
        let visible = view.documentVisibleRect()
        XCTAssertTrue(visible.contains(selection))
    }
}
#endif
