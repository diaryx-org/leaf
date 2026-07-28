//  LinkTargetTests.swift
//
//  What a click can activate. The parsed half (`linkDestinationAtCaret`) is
//  core's and tested there; this covers the lexical `[[…]]` half, which lives in
//  Swift precisely because twig has no node for it — and so has no other test.

import XCTest
import LeafFFI
@testable import LeafUI

final class LinkTargetTests: XCTestCase {
    /// A doc over `source` with the caret parked at byte `offset`.
    private func doc(_ source: String, caret offset: UInt32) throws -> LeafDoc {
        let d = try LeafDoc(source: source, format: "markdown")
        _ = d.setSelectionOffsets(anchor: offset, focus: offset)
        return d
    }

    /// The byte offset of the first occurrence of `needle` in `source`.
    private func offset(of needle: String, in source: String) -> UInt32 {
        UInt32(source.range(of: needle).map { source.utf8.distance(from: source.utf8.startIndex, to: $0.lowerBound) } ?? 0)
    }

    func testFindsWikilinkAroundCaret() throws {
        let src = "see [[notes/a.md]] for more"
        for caret in [offset(of: "[[", in: src), offset(of: "notes", in: src), offset(of: "]]", in: src)] {
            let d = try doc(src, caret: caret)
            XCTAssertEqual(d.activatableTargetAtCaret(wikilinks: true), "[[notes/a.md]]",
                           "caret at \(caret) should be inside the wikilink")
        }
    }

    func testReturnsTheConstructVerbatimIncludingLabel() throws {
        let src = "[[id:6tzwsxg|last week]]"
        let d = try doc(src, caret: 4)
        XCTAssertEqual(d.activatableTargetAtCaret(wikilinks: true), "[[id:6tzwsxg|last week]]")
    }

    func testOffFlagIgnoresWikilinks() throws {
        let src = "see [[notes/a.md]] for more"
        let d = try doc(src, caret: offset(of: "notes", in: src))
        XCTAssertNil(d.activatableTargetAtCaret(wikilinks: false))
    }

    /// Half-open, matching the parsed-link rule: past the closing `]]` is past
    /// the link, which is how a reader places the caret next to one without
    /// navigating.
    func testCaretJustPastTheCloserIsOutside() throws {
        let src = "[[a.md]]x"
        let d = try doc(src, caret: offset(of: "x", in: src))
        XCTAssertNil(d.activatableTargetAtCaret(wikilinks: true))
    }

    func testCaretOutsideAnyWikilink() throws {
        let src = "plain prose, no links"
        let d = try doc(src, caret: 3)
        XCTAssertNil(d.activatableTargetAtCaret(wikilinks: true))
    }

    /// An unclosed `[[` must not reach down the page to pair with an unrelated
    /// `]]` — a wikilink is an inline construct.
    func testDoesNotSpanANewline() throws {
        let src = "[[ unclosed\n\nand ]] later"
        let d = try doc(src, caret: offset(of: "unclosed", in: src))
        XCTAssertNil(d.activatableTargetAtCaret(wikilinks: true))
    }

    func testEmptyTargetIsNotALink() throws {
        let src = "[[   ]]"
        let d = try doc(src, caret: 3)
        XCTAssertNil(d.activatableTargetAtCaret(wikilinks: true))
    }

    /// The scan is over UTF-8 bytes, and core's caret offset is a byte offset —
    /// multi-byte prose ahead of the link must not shift the span it finds.
    func testMultibyteProseBeforeTheLink() throws {
        let src = "café — 日記 [[notes/a.md]] end"
        let d = try doc(src, caret: offset(of: "notes", in: src))
        XCTAssertEqual(d.activatableTargetAtCaret(wikilinks: true), "[[notes/a.md]]")
    }

    /// A real Markdown link wins outright; the wikilink lexer never runs for it.
    func testParsedLinkTakesPrecedence() throws {
        let src = "[last week](./2026-07-20.md)"
        let d = try doc(src, caret: 3)
        XCTAssertEqual(d.activatableTargetAtCaret(wikilinks: true), "./2026-07-20.md")
    }
}
