//  AttributedRowTests.swift
//
//  The one place a run's role/emphasis crosses into AppKit text attributes. The
//  load-bearing invariant is offset alignment (UTF-16 length == sum of run text),
//  since core's caret/hit offsets index into this string; the rest pins the
//  role→font/colour/decoration mapping.

import XCTest
import LeafFFI
@testable import LeafUI

#if canImport(AppKit)
import AppKit
#endif

final class AttributedRowTests: XCTestCase {
    private let theme = EditorTheme.default

    /// Attributes at index 0 of the single-run row built from `r`.
    private func attrs(_ r: Run, code: Bool = false, heading: UInt8? = nil) -> [NSAttributedString.Key: Any] {
        let s = AttributedRow.make(row([r], code: code, heading: heading), theme: theme)
        XCTAssertGreaterThan(s.length, 0)
        return s.attributes(at: 0, effectiveRange: nil)
    }

    func testOffsetsAlignWithConcatenatedRunText() {
        let s = AttributedRow.make(row([mkRun("ab"), mkRun("cde"), mkRun("f")]), theme: theme)
        XCTAssertEqual(s.length, "abcdef".utf16.count)
    }

    func testBoldRunGetsBoldTrait() {
        let font = attrs(mkRun("x", bold: true))[.font] as! LeafFont
        XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(.bold))
    }

    func testLinkRoleColorsAndUnderlines() {
        let a = attrs(mkRun("x", role: "link"))
        XCTAssertEqual(a[.foregroundColor] as? LeafColor, theme.linkColor)
        XCTAssertEqual(a[.underlineStyle] as? Int, NSUnderlineStyle.single.rawValue)
    }

    func testInlineCodeGetsPanelAndMonospace() {
        let a = attrs(mkRun("x", role: "code"))
        XCTAssertEqual(a[.backgroundColor] as? LeafColor, theme.codeBackground)
        let font = a[.font] as! LeafFont
        XCTAssertTrue(font.fontName.contains("Menlo") || font.fontDescriptor.symbolicTraits.contains(.monoSpace))
    }

    func testCodeRowDoesNotDoubleTheBackground() {
        // A code *row* is drawn its own panel by the view; the run must not add one.
        XCTAssertNil(attrs(mkRun("x", role: "code"), code: true)[.backgroundColor])
    }

    func testMarkRoleGetsHighlight() {
        XCTAssertEqual(attrs(mkRun("x", role: "mark"))[.backgroundColor] as? LeafColor, theme.markBackground)
    }

    func testStrikeGetsStrikethrough() {
        XCTAssertEqual(attrs(mkRun("x", strike: true))[.strikethroughStyle] as? Int, NSUnderlineStyle.single.rawValue)
    }

    func testHeadingRowIsBoldAndSized() {
        let font = attrs(mkRun("Title"), heading: 1)[.font] as! LeafFont
        XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(.bold), "a heading line is bold as a whole")
        XCTAssertEqual(font.pointSize, theme.headingSize(1), accuracy: 0.5)
    }
}
