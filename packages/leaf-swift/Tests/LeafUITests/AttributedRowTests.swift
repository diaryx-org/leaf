//  AttributedRowTests.swift
//
//  The one place a run's role/emphasis crosses into AppKit text attributes. The
//  load-bearing invariant is offset alignment (UTF-16 length == sum of run text),
//  since core's caret/hit offsets index into this string; the rest pins the
//  role→font/colour/decoration mapping.

import XCTest
import CoreText
import LeafFFI
@testable import LeafUI

#if canImport(AppKit)
import AppKit
private let boldTrait = NSFontDescriptor.SymbolicTraits.bold
private let monoTrait = NSFontDescriptor.SymbolicTraits.monoSpace
private let italicTrait = NSFontDescriptor.SymbolicTraits.italic
#elseif canImport(UIKit)
import UIKit
// The same traits, under the names UIKit gives them.
private let boldTrait = UIFontDescriptor.SymbolicTraits.traitBold
private let monoTrait = UIFontDescriptor.SymbolicTraits.traitMonoSpace
private let italicTrait = UIFontDescriptor.SymbolicTraits.traitItalic
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
        XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(boldTrait))
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
        XCTAssertTrue(font.fontName.contains("Menlo") || font.fontDescriptor.symbolicTraits.contains(monoTrait))
    }

    func testCodeRowDoesNotDoubleTheBackground() {
        // A code *row* is drawn its own panel by the view; the run must not add one.
        XCTAssertNil(attrs(mkRun("x", role: "code"), code: true)[.backgroundColor])
    }

    func testMarkRoleGetsHighlight() {
        XCTAssertEqual(attrs(mkRun("x", role: "mark"))[.backgroundColor] as? LeafColor, theme.markBackground)
    }

    func testColouredMarkTakesItsOwnWash() {
        // `==🔴 text==`: the name picks the wash, the ink is unchanged, and a
        // name the theme doesn't know falls back rather than drawing nothing.
        let red = attrs(mkRun("x", role: "mark", markColor: "red"))
        XCTAssertEqual(red[.backgroundColor] as? LeafColor, Palette.markBackground(named: "red"))
        XCTAssertNotEqual(red[.backgroundColor] as? LeafColor, theme.markBackground)
        XCTAssertEqual(red[.foregroundColor] as? LeafColor, theme.textColor)
        XCTAssertEqual(
            attrs(mkRun("x", role: "mark", markColor: "chartreuse"))[.backgroundColor] as? LeafColor,
            theme.markBackground,
            "an unknown colour is a plain highlight, not a guess"
        )
    }

    func testTokenPicksTheInkAndLeavesTheRestOfCodeAlone() {
        // A keyword in a fenced block reads in the palette's ink, still in the
        // mono face; an unknown class id — one a newer core might emit — and no
        // token at all both fall back to `codeColor`. A token on prose is
        // ignored: core never produces one, and prose must not take code's ink.
        let keyword = attrs(mkRun("let", role: "code", token: "keyword"), code: true)
        XCTAssertEqual(keyword[.foregroundColor] as? LeafColor, theme.syntaxColors["keyword"])
        XCTAssertNotEqual(keyword[.foregroundColor] as? LeafColor, theme.codeColor)
        let font = keyword[.font] as! LeafFont
        XCTAssertTrue(font.fontName.contains("Menlo") || font.fontDescriptor.symbolicTraits.contains(monoTrait))
        XCTAssertNil(keyword[.backgroundColor], "a code row's panel is the view's, not the run's")

        XCTAssertEqual(
            attrs(mkRun("x", role: "code", token: "meta"), code: true)[.foregroundColor] as? LeafColor,
            theme.codeColor
        )
        XCTAssertEqual(attrs(mkRun("x", role: "code"), code: true)[.foregroundColor] as? LeafColor, theme.codeColor)
        XCTAssertEqual(attrs(mkRun("x", token: "keyword"))[.foregroundColor] as? LeafColor, theme.textColor)
    }

    func testCommentIsItalicOnTopOfItsColour() {
        let comment = attrs(mkRun("// c", role: "code", token: "comment"), code: true)
        let font = comment[.font] as! LeafFont
        XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(italicTrait))
        XCTAssertEqual(comment[.foregroundColor] as? LeafColor, theme.syntaxColors["comment"])
        let string = attrs(mkRun("s", role: "code", token: "string"), code: true)
        XCTAssertFalse((string[.font] as! LeafFont).fontDescriptor.symbolicTraits.contains(italicTrait))
    }

    // MARK: the source view

    /// Attributes at index 0 of a single-run row shaped for the source view.
    private func sourceAttrs(_ r: Run) -> [NSAttributedString.Key: Any] {
        let s = AttributedRow.make(row([r]), theme: theme, source: true)
        XCTAssertGreaterThan(s.length, 0)
        return s.attributes(at: 0, effectiveRange: nil)
    }

    private func isMono(_ font: LeafFont) -> Bool {
        font.fontName.contains("Menlo") || font.fontDescriptor.symbolicTraits.contains(monoTrait)
    }

    func testSourceViewSetsEveryRunInTheMonoFaceAtBodySize() {
        for role in ["body", "delimiter", "link", "h1", "mark", "rule"] {
            let font = sourceAttrs(mkRun("x", role: role))[.font] as! LeafFont
            XCTAssertTrue(isMono(font), "\(role) is not monospaced in the source view")
            XCTAssertEqual(font.pointSize, theme.fontSize, "\(role) was resized in the source view")
        }
        // And not in the rendered one, where prose is the body face.
        XCTAssertFalse(isMono(attrs(mkRun("x"))[.font] as! LeafFont))
    }

    func testSourceViewColoursByRoleAndBoldsAHeadingsText() {
        // The `# ` recedes; the `Title` after it is bold, as its row is rendered.
        XCTAssertEqual(sourceAttrs(mkRun("# ", role: "delimiter"))[.foregroundColor] as? LeafColor,
                       theme.secondaryColor)
        let title = sourceAttrs(mkRun("Title", role: "h1"))
        XCTAssertTrue((title[.font] as! LeafFont).fontDescriptor.symbolicTraits.contains(boldTrait))
        XCTAssertEqual(title[.foregroundColor] as? LeafColor, theme.textColor)
        for level in 2...6 {
            let font = sourceAttrs(mkRun("t", role: "h\(level)"))[.font] as! LeafFont
            XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(boldTrait), "h\(level) is not bold")
        }
        // A link is a link, underlined and in its colour.
        let link = sourceAttrs(mkRun("here", role: "link"))
        XCTAssertEqual(link[.foregroundColor] as? LeafColor, theme.linkColor)
        XCTAssertEqual(link[.underlineStyle] as? Int, NSUnderlineStyle.single.rawValue)
    }

    func testSourceViewDropsTheCodePillAndKeepsTheTokensInk() {
        // A fence's body is a code run per line here; no pill under each.
        let plain = sourceAttrs(mkRun("x", role: "code"))
        XCTAssertNil(plain[.backgroundColor])
        XCTAssertEqual(plain[.foregroundColor] as? LeafColor, theme.codeColor)
        let keyword = sourceAttrs(mkRun("let", role: "code", token: "keyword"))
        XCTAssertNil(keyword[.backgroundColor])
        XCTAssertEqual(keyword[.foregroundColor] as? LeafColor, theme.syntaxColors["keyword"])
        // The rendered view's inline code keeps its pill.
        XCTAssertEqual(attrs(mkRun("x", role: "code"))[.backgroundColor] as? LeafColor, theme.codeBackground)
    }

    func testSourceViewOffsetsStillAlignWithTheRunText() {
        let s = AttributedRow.make(
            row([mkRun("# ", role: "delimiter"), mkRun("Tïtle", role: "h1"), mkRun(" 🍃")]),
            theme: theme, source: true)
        XCTAssertEqual(s.length, "# Tïtle 🍃".utf16.count)
    }

    func testStrikeGetsStrikethrough() {
        XCTAssertEqual(attrs(mkRun("x", strike: true))[.strikethroughStyle] as? Int, NSUnderlineStyle.single.rawValue)
    }

    func testQuoteGutterDrawsClearAndKeepsItsOffsets() {
        // The view paints a real bar over the gutter, so its glyphs must not also
        // ink — but they stay in the string, since core's `caret_ch` counts them.
        let s = AttributedRow.make(row([mkRun("│ ", role: "quote"), mkRun("quoted")]), theme: theme)
        XCTAssertEqual(s.length, "│ quoted".utf16.count, "the gutter still holds its offsets")
        XCTAssertEqual(s.attributes(at: 0, effectiveRange: nil)[.foregroundColor] as? LeafColor, .clear)
        XCTAssertEqual(s.attributes(at: 2, effectiveRange: nil)[.foregroundColor] as? LeafColor, theme.textColor)
    }

    func testQuoteGutterIsSizedToTheThemedIndent() {
        func width(_ text: String, theme: EditorTheme) -> CGFloat {
            let s = AttributedRow.make(row([mkRun(text, role: "quote")]), theme: theme)
            return CGFloat(CTLineGetTypographicBounds(
                CTLineCreateWithAttributedString(s as CFAttributedString), nil, nil, nil))
        }
        XCTAssertEqual(width("│ ", theme: theme), theme.quoteIndent, accuracy: 0.5)
        XCTAssertEqual(width("│ │ ", theme: theme), theme.quoteIndent * 2, accuracy: 0.5,
                       "a second level adds exactly one more gutter")
        // Tracks the theme, not the font's own `│ ` width.
        var wide = theme
        wide.quoteIndent = 40
        XCTAssertEqual(width("│ ", theme: wide), 40, accuracy: 0.5)
    }

    func testSuperscriptIsRaisedAndSetSmaller() {
        let a = attrs(mkRun("1", role: "link", sup: true))
        let font = a[.font] as! LeafFont
        XCTAssertEqual(font.pointSize, theme.fontSize * theme.baselineScale, accuracy: 0.01)
        XCTAssertEqual(a[.baselineOffset] as? CGFloat, font.pointSize * theme.baselineSuperShift)
        // The shift is added to the role, not swapped for it: a footnote
        // reference is still painted and underlined as the link it is.
        XCTAssertEqual(a[.foregroundColor] as? LeafColor, theme.linkColor)
    }

    func testSubscriptIsLoweredAndSetSmaller() {
        let a = attrs(mkRun("2", sub: true))
        let font = a[.font] as! LeafFont
        XCTAssertEqual(font.pointSize, theme.fontSize * theme.baselineScale, accuracy: 0.01)
        XCTAssertEqual(a[.baselineOffset] as? CGFloat, -font.pointSize * theme.baselineSubShift)
    }

    func testOrdinaryRunSitsOnTheBaseline() {
        XCTAssertNil(attrs(mkRun("x"))[.baselineOffset])
    }

    func testRaisedRunScalesWithTheLineItRidesOn() {
        // Measured against the run's own size, not the body's — a reference in
        // an h1 has to stay in proportion to the h1.
        let font = attrs(mkRun("1", sup: true), heading: 1)[.font] as! LeafFont
        XCTAssertEqual(font.pointSize, theme.headingSize(1) * theme.baselineScale, accuracy: 0.01)
    }

    func testRaisedRunKeepsItsOffsetsInTheString() {
        // The load-bearing invariant: `.baselineOffset` shifts glyphs without
        // touching the string, so core's `caret_ch` still indexes it 1:1.
        let s = AttributedRow.make(row([mkRun("A claim"), mkRun("[1]", role: "link", sup: true)]), theme: theme)
        XCTAssertEqual(s.length, "A claim[1]".utf16.count)
    }

    func testHeadingRowIsBoldAndSized() {
        let font = attrs(mkRun("Title"), heading: 1)[.font] as! LeafFont
        XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(boldTrait), "a heading line is bold as a whole")
        XCTAssertEqual(font.pointSize, theme.headingSize(1), accuracy: 0.5)
    }
}
