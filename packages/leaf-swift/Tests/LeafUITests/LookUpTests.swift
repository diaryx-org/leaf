//  LookUpTests.swift
//
//  What the macOS view hands the system's Look Up: the words as they are
//  drawn — the run's own font, on the drawn line's baseline, at the zoom —
//  so the yellow highlight the system sets lands exactly on them.

#if canImport(AppKit) && !targetEnvironment(macCatalyst)
import AppKit
import XCTest
import LeafFFI
@testable import LeafUI

final class LookUpTests: XCTestCase {
    private func editor(_ source: String) throws -> LeafTextView {
        let view = LeafTextView(doc: try LeafDoc(source: source, format: "markdown"), theme: .default)
        view.frame = NSRect(x: 0, y: 0, width: 600, height: 400)
        view.layout()
        return view
    }

    private func byteRange(of needle: String, in view: LeafTextView) -> (Int, Int) {
        let source = view.sourceText()
        let r = source.range(of: needle)!
        let from = source.utf8.distance(from: source.startIndex, to: r.lowerBound)
        return (from, from + needle.utf8.count)
    }

    func testTheWordUnderAPointIsCoresWordAndMovesNothing() throws {
        let view = try editor("hello wonderful world\n")
        let caret = view.doc.caretOffset()
        let word = try XCTUnwrap(view.doc.wordRangeAt(off: 8))
        XCTAssertEqual(view.doc.textInRange(from: word.start, to: word.end), "wonderful")
        XCTAssertEqual(view.doc.caretOffset(), caret)
        XCTAssertNil(view.doc.selectedText())
    }

    func testTheWordIsHandedOverInTheFontItIsDrawnIn() throws {
        let view = try editor("plain **bold** text\n")
        let (from, to) = byteRange(of: "bold", in: view)
        let (text, _) = try XCTUnwrap(view.lookUpPresentation(fromByte: from, toByte: to))
        XCTAssertEqual(text.string, "bold")
        let font = try XCTUnwrap(text.attribute(.font, at: 0, effectiveRange: nil) as? NSFont)
        XCTAssertTrue(font.fontDescriptor.symbolicTraits.contains(.bold), "the drawn run's face, not a plain string")
        XCTAssertEqual(font.pointSize, view.theme.fontSize, accuracy: 0.01)
    }

    func testTheBaselineSitsOnTheDrawnLine() throws {
        let view = try editor("hello world\n")
        let (from, to) = byteRange(of: "world", in: view)
        let (_, baseline) = try XCTUnwrap(view.lookUpPresentation(fromByte: from, toByte: to))
        let box = try XCTUnwrap(view.layoutEngine.rangeRects(fromByte: from, toByte: to, in: view.doc).first)
        XCTAssertEqual(baseline.x, box.minX, accuracy: 0.5, "starts at the word's first glyph")
        // Below the line's top by the font's ascent: inside the line box, in
        // its upper part — not at its top (where a guessed origin put it) nor
        // a line further down.
        XCTAssertGreaterThan(baseline.y, box.minY + view.theme.fontSize * 0.5)
        XCTAssertLessThan(baseline.y, box.maxY)
    }

    func testTheZoomScalesBothTheOriginAndTheFont() throws {
        let view = try editor("hello world\n")
        let (from, to) = byteRange(of: "world", in: view)
        view.zoom = .scale(2)
        // The layout is unzoomed points (and re-centred in the narrower
        // viewport a zoom leaves); the system is handed view points.
        let box = try XCTUnwrap(view.layoutEngine.rangeRects(fromByte: from, toByte: to, in: view.doc).first)
        let (text, zoomed) = try XCTUnwrap(view.lookUpPresentation(fromByte: from, toByte: to))
        XCTAssertEqual(zoomed.x, box.minX * 2, accuracy: 0.5)
        XCTAssertGreaterThan(zoomed.y, (box.minY + view.theme.fontSize * 0.5) * 2)
        XCTAssertLessThan(zoomed.y, box.maxY * 2)
        let font = try XCTUnwrap(text.attribute(.font, at: 0, effectiveRange: nil) as? NSFont)
        XCTAssertEqual(font.pointSize, view.theme.fontSize * 2, accuracy: 0.01)
    }

    func testASelectionAcrossBlocksIsJoinedAsTheVisibleTextJoinsThem() throws {
        let view = try editor("first para\n\n- second item\n")
        let (from, _) = byteRange(of: "para", in: view)
        let (_, to) = byteRange(of: "second", in: view)
        let (text, _) = try XCTUnwrap(view.lookUpPresentation(fromByte: from, toByte: to))
        // The list's bullet is decoration drawn in front of the item, not text.
        XCTAssertEqual(text.string, "para\nsecond")
    }

    func testNothingToLookUpBetweenWords() throws {
        let view = try editor("a -- b\n")
        XCTAssertNil(view.doc.wordRangeAt(off: 3))
    }
}
#endif
